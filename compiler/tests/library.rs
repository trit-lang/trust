//! The standard library, as far as it goes (Language Ch. 5, Ch. 0 §3.6).
//!
//! What is here is what a compiler written in this language would reach for
//! first, which is how it was chosen: `lower.rs` names `String` 217 times and
//! `HashMap` 101, and those two are the list.

use trustc::lang;

fn refusal(src: &str) -> Option<String> {
    lang::compile(src)
        .err()
        .map(|errs| errs.iter().map(|e| e.message.clone()).collect())
}

#[test]
fn a_type_alias_is_only_a_name() {
    // Ch. 0 §3.6: `Text` and `Vec<char>` are the same type, so a `Text` may
    // be passed where a `&[char]` is wanted and a `&str` beside it.
    let src = "type Text = Vec<char>;\n\
               fn wide(s: &str) -> taddr { s.len() }\n\
               fn main() -> t27 {\n\
                   let mut t: Text = Vec::new();\n\
                   t.push_str(\"abc\");\n\
                   wide(&t) as t27\n\
               }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn an_alias_may_name_another() {
    let src = "type A = Vec<char>;\ntype B = A;\n\
               fn main() -> t27 { let b: B = Vec::new(); b.len() as t27 }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn an_alias_that_names_itself_is_refused() {
    let why = refusal("type A = B;\ntype B = A;\nfn main() -> t27 { 0 }\n").expect("refused");
    assert!(why.contains("names itself"), "{why}");
}

#[test]
fn an_alias_takes_no_parameters_in_draft_zero_one() {
    let why = refusal("type Pair<T> = T;\nfn main() -> t27 { 0 }\n").expect("refused");
    assert!(why.contains("no parameters"), "{why}");
}

#[test]
fn a_string_is_owned_growable_text() {
    // `String` is the prelude's name for `Vec<char>`, so everything a `Vec`
    // can do it can do, and everything a `&str` can do it can do through the
    // coercion (Ch. 5 §2.6).
    let src = "fn main() -> t27 {\n\
               \x20   let mut s: String = \"hello\".to_string();\n\
               \x20   s.push_str(\", world\");\n\
               \x20   let mut n = 0;\n\
               \x20   if s.starts_with(\"hello\") { n += 1; }\n\
               \x20   if s.contains(\"o, w\") { n += 10; }\n\
               \x20   if \"abc\".eq(\"abc\") { n += 100; }\n\
               \x20   if \"abc\".eq(\"abd\") { n += 1000; }\n\
               \x20   for c in s { if c == 'o' { n += 10000; } }\n\
               \x20   n\n}\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_vec_of_char_answers_a_strs_methods() {
    // Passing one already worked. Calling one is the same coercion asked at
    // the receiver instead of at an argument.
    let src = "fn main() -> bool {\n\
               \x20   let mut s: Vec<char> = Vec::new();\n\
               \x20   s.push_str(\"abc\");\n\
               \x20   s.starts_with(\"ab\")\n}\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn the_prelude_defines_each_name_once() {
    // Two `struct Map` coexisted in the prelude — the iterator adaptor and a
    // hash map — and nothing said so, because a program's own duplicates are
    // caught while resolving modules and the prelude is not resolved. What
    // it cost was a field lookup finding the other one's fields (G9.32).
    use trustc::lang::ast::Item;
    let file = trustc::lang::parse::parse(trustc::lang::PRELUDE).expect("the prelude parses");
    let mut seen: Vec<&str> = Vec::new();
    let mut twice: Vec<&str> = Vec::new();
    for item in &file.items {
        let name = match item {
            Item::Fn(f) => &f.name,
            Item::Struct(s) => &s.name,
            Item::Enum(e) => &e.name,
            Item::Trait(t) => &t.name,
            Item::Const(c) => &c.name,
            Item::Alias(a) => &a.name,
            _ => continue,
        };
        if seen.contains(&name.as_str()) {
            twice.push(name);
        }
        seen.push(name);
    }
    assert!(twice.is_empty(), "defined twice in the prelude: {twice:?}");
}

#[test]
fn a_primitive_satisfies_eq_and_ord() {
    // Ch. 4 §5.3 gives `==` its meaning through `Eq` for a nominal type; a
    // primitive has it from Ch. 1 §4 directly, so a bound asking for `Eq` is
    // satisfied by one without an impl nobody could usefully write.
    let src = "fn same<K: Eq>(a: &K, b: &K) -> bool { *a == *b }\n\
               fn main() -> t27 { let x = 1; let y = 1; if same(&x, &y) { 1 } else { 0 } }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_hash_map_holds_what_is_put_in_it() {
    let src = "fn main() -> t27 {\n\
               \x20   let mut m: HashMap<t27, t27> = HashMap::new();\n\
               \x20   m.insert(1, 10);\n\
               \x20   m.insert(65, 650);\n\
               \x20   m.insert(1, 11);\n\
               \x20   let a = 1;\n\
               \x20   let c = 3;\n\
               \x20   let hit = if m.has(&a) { 1 } else { 0 };\n\
               \x20   let miss = if m.has(&c) { 1 } else { 0 };\n\
               \x20   m.len() as t27 * 100 + hit * 10 + miss\n}\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_method_name_the_language_uses_is_free_on_another_type() {
    // `insert` is a `Vec`'s, and the gate used to ask only whether *any*
    // type had a method of that name — so a program could not define one.
    let src = "struct Bag { n: t27 }\n\
               impl Bag { fn insert(&mut self, x: t27) { self.n += x; } }\n\
               fn main() -> t27 { let mut b = Bag { n: 0 }; b.insert(3); b.n }\n";
    assert_eq!(refusal(src), None);
}
