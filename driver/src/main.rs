//! `trust` — compile a Trust program and run it, with nothing on the disk.
//!
//! `trustc compile` writes assembly and stops, `tritium asm` turns assembly
//! into an image and `tritium run` executes one. That is three commands and
//! two temporary files to answer "what does this program print", which is the
//! question asked most often. This asks it in one.
//!
//! It is a separate crate because the boundary it crosses is real: the
//! compiler emits text and knows nothing about a machine, and the machine
//! knows nothing about Trust. Joining them is a third thing's job.

use std::process::ExitCode;

use trustc::{codegen, lang, tir};

const USAGE: &str = "\
trust — compile a Trust program and run it

usage:
    trust run <file.tr> [--stats]      compile, link the runtime, execute;
                                       stdin to stdout
    trust asm <file.tr>                print the assembly it would run
    trust check <file.tr>              report what is wrong with it, and stop
    trust lex <file.tr>                print its tokens, one per line
    trust ast <file.tr>                read the file as one expression and
                                       print its tree
    trust item <file.tr>               read the file as one function and
                                       print its tree
    trust bundle <file.tr> [--prelude] the whole module tree as one text,
                                       for a compiler that cannot open files;
                                       with the prelude when asked
    trust build <file.tr>              the whole program as TIR
    trust modules <file.tr>            each module of it, and its token count
    trust file <file.tr>               every item in it, as a tree
    trust symbols <file.tr>            every name the program defines
    trust uses <file.tr>               what every `use` in it reaches
    trust flat <file.tr>               the program as one list of items, with
                                       every name resolved

    --time                             report what each phase of the compile
                                       cost, on stderr

The file named is the program's **root**; what else is compiled is what its
`mod` declarations say (Ch. 6 §1).

`check` compiles no further than it must to know: it is what an editor runs
on every save, and what `trust-lsp` reports. Its output is one diagnostic per
line, `file:line:column: message`, and it exits 1 if there was one.

`run` exits with the program's own status, so it composes with a shell the
way any other program does. `--stats` reports instructions retired (ISA §2.3)
on stderr, which is the number every measurement in docs/ is quoted in.
";

/// The hand-written bottom of the stack: `putchar`, the cycle counter and the
/// allocator, which are the target's and not the language's (Ch. 5 §2.1).
const RUNTIME: &str = include_str!("../../examples/trisc/runtime.t27");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(path)) = (args.first(), args.get(1)) else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let stats = args.iter().any(|a| a == "--stats");
    let timed = args.iter().any(|a| a == "--time");

    // One token per line, in the form `bootstrap/main.tr` prints — so that
    // the lexer written in Trust can be held to this one (see
    // `scripts/bootstrap.sh`). It reads the file and stops: lexing does not
    // need the program to be a program, and a corpus that exercises the
    // punctuation is not one.
    if cmd == "lex" {
        return match std::fs::read_to_string(path) {
            Ok(src) => print_tokens(&src),
            Err(e) => {
                eprintln!("trust: cannot read `{path}`: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // Each module of a program and how many tokens it holds — the line
    // `bootstrap/whole.tr` prints, so that the two can be compared on a
    // *program* rather than on a file.
    if cmd == "modules" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        for source in &program.sources {
            let name = match source.path.is_empty() {
                true => "-".to_string(),
                false => source.path.join("."),
            };
            match lang::lex::lex(&source.text) {
                Ok(t) => println!("mod {name} {}", t.len()),
                Err(e) => println!("mod {name} error {}", e.span.lo),
            }
        }
        return ExitCode::SUCCESS;
    }

    // Pass one of Ch. 6 §4: every name the program defines, and what it
    // would be called. It is the first thing a second implementation of the
    // chapter can be held to, because it needs no types and no lowering.
    if cmd == "symbols" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        for (kind, name, public) in lang::modules::symbols(&program) {
            let seen = match public {
                true => "pub",
                false => "priv",
            };
            println!("{kind} {name} {seen}");
        }
        return ExitCode::SUCCESS;
    }

    // Pass two of Ch. 6 §4: what every `use` reaches. A refusal is reported
    // as the rule that refused it and not as its wording, because two
    // implementations should agree about the language and need not agree
    // about a sentence.
    if cmd == "uses" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        for (module, written, got) in lang::modules::uses(&program) {
            match got {
                Ok(full) => println!("{module} {written} -> {full}"),
                Err(why) => println!("{module} {written} ! {why}"),
            }
        }
        return ExitCode::SUCCESS;
    }

    // Ch. 4, reported: what type every binding in the file's own functions
    // turned out to have. A `let` is where inference is visible — it is the
    // thing `let n = 1;` does not say — and it needs no module machinery,
    // so the question is asked of one file.
    if cmd == "types" {
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("trust: cannot read {path}");
            return ExitCode::FAILURE;
        };
        let analysis = lang::analyze(&src);
        let file = match lang::parse::parse(&src) {
            Ok(f) => f,
            Err(e) => {
                println!("error {}", e.span.lo);
                return ExitCode::SUCCESS;
            }
        };
        for item in &file.items {
            let lang::ast::Item::Fn(f) = item else {
                continue;
            };
            // A generic function is typed once per instantiation, so there
            // is no one answer to print (Ch. 4 §2.5).
            if !f.generics.is_empty() {
                continue;
            }
            let Some(body) = &f.body else { continue };
            print_bindings(&f.name, body, &analysis.types);
        }
        return ExitCode::SUCCESS;
    }

    // Ch. 4, refused: which functions do not type-check, and by which rule.
    //
    // Reported per *function* and by rule rather than by wording or by
    // position: two implementations should agree about the language, and a
    // second one has no spans to point with (`bootstrap/`'s tree carries
    // none). What both can say is "this function is refused, and it is a
    // type that does not match".
    if cmd == "agree" {
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("trust: cannot read {path}");
            return ExitCode::FAILURE;
        };
        let analysis = lang::analyze(&src);
        let file = match lang::parse::parse(&src) {
            Ok(f) => f,
            Err(e) => {
                println!("error {}", e.span.lo);
                return ExitCode::SUCCESS;
            }
        };
        for item in &file.items {
            let lang::ast::Item::Fn(f) = item else {
                continue;
            };
            if !f.generics.is_empty() || f.body.is_none() {
                continue;
            }
            let span = f.span;
            let why = analysis
                .errors
                .iter()
                .find(|e| e.span.lo >= span.lo && e.span.hi <= span.hi)
                .map(|e| match e.message.contains("expected") {
                    true => "mismatch",
                    false => "other",
                });
            match why {
                Some(class) => println!("{} {class}", f.name),
                None => println!("{} ok", f.name),
            }
        }
        return ExitCode::SUCCESS;
    }

    // Ch. 2, reported: what every nominal type the program defines is laid
    // out as. Sizes and offsets are facts about types and need no inference,
    // which makes this the first part of the middle a second implementation
    // can be held to.
    if cmd == "layout" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        let (user, errors) = lang::modules::resolve(&program);
        if !errors.is_empty() {
            for e in &errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        // The prelude is part of the program (Ch. 5), and its types are laid
        // out by the same rules — but a report of *this* program's types is
        // what a second implementation can answer, so the merge is skipped
        // and what is printed is what the file defines.
        match lang::lower::layouts(&user) {
            Ok(ls) => {
                for (name, l) in ls {
                    // The niche *count* is not printed. It is 3^n for n up
                    // to 27 trits, computed here in 128-bit arithmetic that
                    // Trust does not have; what both implementations can be
                    // held to is every decision made from it (G9.49).
                    print!("{name} size={} align={}", l.size, l.align);
                    for off in &l.offsets {
                        print!(" +{off}");
                    }
                    if let Some(e) = &l.enum_layout {
                        print!(" tag={}", tag_word(&e.tag));
                        for d in &e.discriminants {
                            print!(" ={d}");
                        }
                        for v in &e.variant_offsets {
                            print!(" [");
                            for (i, off) in v.iter().enumerate() {
                                if i > 0 {
                                    print!(" ");
                                }
                                print!("+{off}");
                            }
                            print!("]");
                        }
                    }
                    println!();
                }
                return ExitCode::SUCCESS;
            }
            Err(errs) => {
                for e in &errs {
                    eprintln!("trust: {}", e.message);
                }
                return ExitCode::FAILURE;
            }
        }
    }

    // The whole program as TIR: `trustc build` reads one file, and a
    // program is a tree of them (Ch. 6 §1).
    if cmd == "build" {
        let build = lang::build(std::path::Path::new(path));
        match &build.module {
            Some(m) if build.errors.is_empty() => {
                print!("{}", trustc::tir::print_module(m));
                return ExitCode::SUCCESS;
            }
            _ => {
                report(&build);
                return ExitCode::FAILURE;
            }
        }
    }

    // Pass three of Ch. 6 §4: the whole program as one flat list of items,
    // every name in it resolved. It is what the rest of the compiler has
    // always been given, and it is the last question about this chapter that
    // can be asked without asking about types.
    if cmd == "flat" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        let (file, errors) = lang::modules::resolve(&program);
        for e in &errors {
            println!("error {}", e.message);
        }
        print_items(&file);
        return ExitCode::SUCCESS;
    }

    // The whole program as one text, for something that reads stdin.
    //
    // A compiler written in Trust cannot open a file: the machine has a
    // character port and no filesystem (ISA §2.2), and giving it one would
    // be putting an operating system inside a machine. So finding the files
    // stays this side of the line — which is where it belongs, since which
    // files are compiled is a fact about a *build* — and the compiler is
    // handed the program it is to compile.
    if cmd == "bundle" {
        let program = lang::modules::load(std::path::Path::new(path));
        if !program.errors.is_empty() {
            for e in &program.errors {
                eprintln!("trust: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
        // The prelude, when it is asked for. It is not a module and has no
        // path (Ch. 6 §3.3), so it is sent under a name no module path can
        // be: `#` is not an identifier character.
        //
        // One copy, here, rather than a second in `bootstrap/`. A compiler
        // written in Trust is handed the library it compiles against for
        // the same reason it is handed the files: finding them is a fact
        // about a build, and a second copy is a second thing to keep the
        // same as the first.
        if args.iter().any(|a| a == "--prelude") {
            println!("mod #prelude {}", lang::PRELUDE.chars().count());
            print!("{}", lang::PRELUDE);
        }
        for source in &program.sources {
            // Length-prefixed and in characters, which is what a `str` is
            // indexed by (Ch. 5 §1.1): no escaping, and nothing a program
            // could write can end a section early.
            let name = match source.path.is_empty() {
                true => "-".to_string(),
                false => source.path.join("."),
            };
            println!("mod {name} {}", source.text.chars().count());
            print!("{}", source.text);
        }
        return ExitCode::SUCCESS;
    }

    // Every item in a file, as a tree.
    if cmd == "file" {
        return match std::fs::read_to_string(path) {
            Ok(src) => print_file(&src),
            Err(e) => {
                eprintln!("trust: cannot read `{path}`: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // The file, read as one function, printed as a tree.
    if cmd == "item" {
        return match std::fs::read_to_string(path) {
            Ok(src) => print_item(&src),
            Err(e) => {
                eprintln!("trust: cannot read `{path}`: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // The file, read as one expression, printed as a tree — the form
    // `bootstrap/main.tr` prints, so that the parser written in Trust can be
    // held to this one.
    if cmd == "ast" {
        return match std::fs::read_to_string(path) {
            Ok(src) => print_expr(&src),
            Err(e) => {
                eprintln!("trust: cannot read `{path}`: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // A program is a tree of files (Ch. 6 §1), and the one named is its
    // root: what else is compiled is what its `mod` declarations say.
    let build = lang::build(std::path::Path::new(path));
    if !build.errors.is_empty() {
        report(&build);
        return ExitCode::FAILURE;
    }

    // `check` stops before code generation, because nothing after the
    // frontend can tell a program anything about itself.
    if cmd == "check" {
        return ExitCode::SUCCESS;
    }

    let module = build.module.expect("no errors means a module");
    let asm = match assemble_module(module, timed) {
        Ok(a) => a,
        Err(code) => return code,
    };

    match cmd.as_str() {
        "asm" => {
            print!("{asm}");
            ExitCode::SUCCESS
        }
        "run" => run(&asm, stats),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("trust: unknown command `{other}`\n");
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Everything wrong with a program, and nothing else.
///
/// The frontend is where every diagnostic a reader can act on comes from:
/// what follows it is legalization and code generation, whose errors are
/// about this compiler rather than about the program. So `check` runs the
/// frontend and the verifier, and stops.
fn report(build: &lang::Build) {
    // One `LineMap` per file, because a span's offsets are into the file it
    // names and no other (Ch. 6 §1).
    let maps: Vec<lang::LineMap> = build
        .program
        .sources
        .iter()
        .map(|s| lang::LineMap::new(&s.text))
        .collect();
    for e in &build.errors {
        let Some(src) = build.program.sources.get(e.span.file as usize) else {
            println!("trust: {}", e.message);
            continue;
        };
        // `file:line:column`, which is what an editor's error list, a
        // terminal's hyperlink and `vim +N` all read.
        let at = maps[e.span.file as usize].pos(e.span.lo);
        println!("{}:{}:{}: {}", src.label(), at.line, at.column, e.message);
    }
}

/// Every token of a file, one per line.
fn print_tokens(src: &str) -> ExitCode {
    use lang::lex::Tok;
    let toks = match lang::lex::lex(src) {
        Ok(t) => t,
        // Where, not why. The lexer written in Trust is held to agreeing on
        // what is wrong and on the character it is at, and not on how each
        // of them says it: wording is not what a second implementation
        // checks.
        Err(e) => {
            println!("error {}", e.span.lo);
            return ExitCode::SUCCESS;
        }
    };
    for (t, _) in &toks {
        match t {
            Tok::Ident(n) => println!("ident {n}"),
            Tok::Kw(k) => println!("kw {k}"),
            Tok::Op(o) => println!("op {o}"),
            Tok::Int(v) => println!("int {v}"),
            Tok::StrLit(cs) => {
                let text: String = cs
                    .iter()
                    .filter_map(|c| char::from_u32(*c as u32))
                    .collect();
                println!("str {text}");
            }
            Tok::TritLit(t) => println!("trit {}", t.to_i8()),
            Tok::CharLit(v) => println!("char {v}"),
            Tok::Lifetime(l) => println!("lifetime {l}"),
            Tok::Eof => println!("eof"),
        }
    }
    ExitCode::SUCCESS
}

/// One expression, as a tree.
///
/// The expression is wrapped in a function so that the ordinary parser reads
/// it, and the tail of that function is what is printed. Every operator is a
/// prefix and every child is parenthesized, so the shape is unambiguous and
/// two parsers can be compared on it without agreeing on how to print.
fn print_expr(src: &str) -> ExitCode {
    let wrapped = format!("fn e() -> t27 {{ {} }}", src.trim_end());
    let file = match lang::parse::parse(&wrapped) {
        Ok(f) => f,
        Err(e) => {
            // Where, not why — and in the input's own coordinates, which is
            // the wrapper's length behind.
            let prefix = "fn e() -> t27 { ".chars().count() as u32;
            println!("error {}", e.span.lo.saturating_sub(prefix));
            return ExitCode::SUCCESS;
        }
    };
    let Some(lang::ast::Item::Fn(f)) = file.items.first() else {
        return ExitCode::FAILURE;
    };
    let Some(tail) = f.body.as_ref().and_then(|b| b.tail.as_ref()) else {
        println!("error 0");
        return ExitCode::SUCCESS;
    };
    let mut out = String::new();
    show_expr(tail, &mut out);
    println!("{out}");
    ExitCode::SUCCESS
}

/// One function, as a tree.
/// A generic parameter list, or nothing when there is none (Ch. 4 §2.1).
fn show_generics(gs: &[lang::ast::GenericParam], out: &mut String) {
    use lang::ast::GenericParam;
    if gs.is_empty() {
        return;
    }
    out.push('<');
    for (i, g) in gs.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        match g {
            GenericParam::Type { name, bounds } => {
                out.push_str(name);
                for (k, b) in bounds.iter().enumerate() {
                    out.push(if k == 0 { ':' } else { '+' });
                    show_bound(b, out);
                }
            }
            GenericParam::Const { name, ty } => {
                out.push_str(&format!("const {name}:{}", written_ty(ty)));
            }
        }
    }
    out.push('>');
}

/// One bound: a trait, its type arguments, and what it binds (Ch. 4 §1.7).
fn show_bound(b: &lang::ast::Bound, out: &mut String) {
    out.push_str(&b.name);
    if b.args.is_empty() && b.assoc.is_empty() {
        return;
    }
    out.push('<');
    let mut first = true;
    for a in &b.args {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&written_ty(a));
    }
    for (k, v) in &b.assoc {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!("{k}={}", written_ty(v)));
    }
    out.push('>');
}

fn show_fn(f: &lang::ast::FnItem, out: &mut String) {
    out.push_str(&format!("(fn {} ", f.name));
    if !f.generics.is_empty() {
        show_generics(&f.generics, out);
        out.push(' ');
    }
    out.push('(');
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{}:{}", p.name, written_ty(&p.ty)));
    }
    out.push_str(") ");
    out.push_str(&match &f.ret {
        Some(t) => written_ty(t),
        None => "()".to_string(),
    });
    // What the caller must have established (Ch. 4 §2.8). It is written in
    // the same `where` clause as the type bounds and is a different kind of
    // thing, so it is printed as one.
    for p in &f.requires {
        out.push_str(" (requires ");
        show_expr(p, out);
        out.push(')');
    }
    if let Some(body) = &f.body {
        out.push(' ');
        show_block(body, out);
    }
    out.push(')');
}

fn print_item(src: &str) -> ExitCode {
    let file = match lang::parse::parse(src) {
        Ok(f) => f,
        Err(e) => {
            println!("error {}", e.span.lo);
            return ExitCode::SUCCESS;
        }
    };
    let Some(lang::ast::Item::Fn(f)) = file.items.first() else {
        println!("error 0");
        return ExitCode::SUCCESS;
    };
    let mut out = String::new();
    show_fn(f, &mut out);
    println!("{out}");
    ExitCode::SUCCESS
}

/// Every item of a file, one per line.
fn print_file(src: &str) -> ExitCode {
    let file = match lang::parse::parse(src) {
        Ok(f) => f,
        Err(e) => {
            println!("error {}", e.span.lo);
            return ExitCode::SUCCESS;
        }
    };
    print_items(&file);
    ExitCode::SUCCESS
}

/// Every item of a file, one per line — whether it was read from one file or
/// resolved out of a program of them.
fn print_items(file: &lang::ast::File) {
    use lang::ast::Item;
    for item in &file.items {
        let mut out = String::new();
        // An attribute is written before the item and is printed before it.
        if repr_linear(item) {
            out.push_str("(repr linear) ");
        }
        for d in derives_of(item) {
            out.push_str(&format!("(derive {d}) "));
        }
        match item {
            Item::Fn(f) => show_fn(f, &mut out),
            Item::Struct(s) => {
                out.push_str(&format!("(struct {} ", s.name));
                if !s.generics.is_empty() {
                    show_generics(&s.generics, &mut out);
                    out.push(' ');
                }
                out.push('(');
                for (i, f) in s.fields.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    out.push_str(&format!("{}:{}", f.name, written_ty(&f.ty)));
                }
                out.push_str("))");
            }
            Item::Enum(e) => {
                out.push_str(&format!("(enum {}", e.name));
                if !e.generics.is_empty() {
                    out.push(' ');
                    show_generics(&e.generics, &mut out);
                }
                for v in &e.variants {
                    out.push_str(&format!(" ({}", v.name));
                    for f in &v.fields {
                        out.push_str(&format!(" {}", written_ty(&f.ty)));
                    }
                    out.push(')');
                }
                out.push(')');
            }
            Item::Trait(t) => {
                out.push_str(&format!("(trait {}", t.name));
                if !t.params.is_empty() {
                    out.push_str(&format!(" <{}>", t.params.join(" ")));
                }
                for s in &t.supertraits {
                    out.push_str(&format!(" :{s}"));
                }
                for a in &t.assoc {
                    out.push_str(&format!(" (type {a})"));
                }
                for (n, ty) in &t.consts {
                    out.push_str(&format!(" (const {n}:{})", written_ty(ty)));
                }
                for m in &t.methods {
                    out.push(' ');
                    show_fn(m, &mut out);
                }
                out.push(')');
            }
            Item::Mod(m) => out.push_str(&format!("(mod {})", m.name)),
            Item::Use(u) => out.push_str(&format!("(use {})", u.segments.join("::"))),
            Item::Alias(a) => out.push_str(&format!("(type {} {})", a.name, written_ty(&a.ty))),
            Item::Const(c) => {
                out.push_str(&format!("(const {}:{} ", c.name, written_ty(&c.ty)));
                show_expr(&c.value, &mut out);
                out.push(')');
            }
            Item::Impl(i) => {
                out.push_str("(impl ");
                if !i.generics.is_empty() {
                    show_generics(&i.generics, &mut out);
                    out.push(' ');
                }
                if let Some(t) = &i.trait_name {
                    out.push_str(t);
                    if !i.trait_args.is_empty() {
                        let args: Vec<String> = i.trait_args.iter().map(written_ty).collect();
                        out.push_str(&format!("<{}>", args.join(",")));
                    }
                    out.push_str(" for ");
                }
                out.push_str(&i.self_ty);
                if !i.self_args.is_empty() {
                    let args: Vec<String> = i.self_args.iter().map(written_ty).collect();
                    out.push_str(&format!("<{}>", args.join(",")));
                }
                for m in &i.methods {
                    out.push(' ');
                    show_fn(m, &mut out);
                }
                out.push(')');
            }
            other => out.push_str(&format!("<{:?}>", std::mem::discriminant(other))),
        }
        println!("{out}");
    }
}

/// Every `let` in a block, in source order, and what its name turned out to
/// be. A `let` with a pattern is not here: what it binds is the pattern's,
/// and nothing records those yet.
fn print_bindings(what: &str, b: &lang::ast::Block, types: &lang::lower::Noted) {
    use lang::ast::Stmt;
    for stmt in &b.stmts {
        match stmt {
            Stmt::Let {
                name,
                name_span,
                value,
                pattern,
                ..
            } => {
                // A `let` with a pattern binds what the *pattern* binds.
                // Those are reported to an editor (they are noted) and not
                // here: the second implementation infers types and does not
                // yet infer a pattern's, and a report neither can check is
                // a report worth nothing.
                if pattern.is_none() {
                    let ty = types.exact(*name_span).unwrap_or("?");
                    println!("{what} {name} {ty}");
                }
                bindings_in_expr(what, value, types);
            }
            Stmt::Expr(e) => bindings_in_expr(what, e, types),
        }
    }
    if let Some(t) = &b.tail {
        bindings_in_expr(what, t, types);
    }
}

/// The blocks an expression holds, in source order.
fn bindings_in_expr(what: &str, e: &lang::ast::Expr, types: &lang::lower::Noted) {
    use lang::ast::Expr;
    match e {
        Expr::Block(b) => print_bindings(what, b, types),
        Expr::If(c, then, other, _) => {
            bindings_in_expr(what, c, types);
            print_bindings(what, then, types);
            if let Some(o) = other {
                bindings_in_expr(what, o, types);
            }
        }
        Expr::While(c, b, _) => {
            bindings_in_expr(what, c, types);
            print_bindings(what, b, types);
        }
        Expr::For(_, _, it, b, _) => {
            bindings_in_expr(what, it, types);
            print_bindings(what, b, types);
        }
        Expr::Loop(b, _) => print_bindings(what, b, types),
        Expr::Match(sc, arms, _) => {
            bindings_in_expr(what, sc, types);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    bindings_in_expr(what, g, types);
                }
                bindings_in_expr(what, &arm.body, types);
            }
        }
        other => lang::for_each_child(other, &mut |c| bindings_in_expr(what, c, types)),
    }
}

/// A type as its text, which is all either parser keeps of one here.
/// How a discriminant is stored, in one word (Ch. 2 §5).
fn tag_word(t: &trustc::layout::Tag) -> String {
    use trustc::layout::Tag;
    match t {
        Tag::None => "none".to_string(),
        Tag::TritShaped => "trit".to_string(),
        // In trytes, which is what a second implementation can say: the
        // name of the integer type is this compiler's spelling.
        Tag::Direct { ty, offset } => {
            format!("direct:{}:{offset}", ty.trits().div_ceil(9))
        }
        Tag::Niche {
            untagged,
            offset,
            used,
            ..
        } => format!("niche:{untagged}:{offset}:{used}"),
    }
}

/// Whether an item was written `#[repr(linear)]` (Ch. 2 §1).
fn repr_linear(i: &lang::ast::Item) -> bool {
    use lang::ast::{Item, Repr};
    match i {
        Item::Struct(s) => s.repr == Repr::Linear,
        Item::Enum(e) => e.repr == Repr::Linear,
        _ => false,
    }
}

/// What an item derives, in the order written (Ch. 4 §6).
fn derives_of(i: &lang::ast::Item) -> &[String] {
    use lang::ast::Item;
    match i {
        Item::Struct(s) => &s.derives,
        Item::Enum(e) => &e.derives,
        _ => &[],
    }
}

fn written_ty(t: &lang::ast::Ty) -> String {
    use lang::ast::Ty;
    match t {
        Ty::Name(n, _) => n.clone(),
        Ty::Unit(_) => "()".to_string(),
        Ty::Ref(inner, true, _) => format!("&mut {}", written_ty(inner)),
        Ty::Ref(inner, false, _) => format!("&{}", written_ty(inner)),
        Ty::Slice(inner, _) => format!("[{}]", written_ty(inner)),
        Ty::SelfTy(_) => "Self".to_string(),
        Ty::Assoc(base, name, _) => format!("{}::{name}", written_ty(base)),
        Ty::App(n, args, _) => {
            let args: Vec<String> = args.iter().map(written_ty).collect();
            format!("{n}<{}>", args.join(","))
        }
        other => format!("{other:?}"),
    }
}

fn show_jump(word: &str, v: Option<&lang::ast::Expr>, out: &mut String) {
    out.push_str(&format!("({word}"));
    if let Some(e) = v {
        out.push(' ');
        show_expr(e, out);
    }
    out.push(')');
}

fn show_block(b: &lang::ast::Block, out: &mut String) {
    use lang::ast::Stmt;
    out.push_str("(block");
    for s in &b.stmts {
        out.push(' ');
        match s {
            Stmt::Let {
                mutable,
                name,
                ty,
                value,
                pattern,
                ..
            } => {
                // A `let` with a pattern binds what the *pattern* binds; the
                // name is the compiler's and names nothing a reader wrote.
                if let Some(p) = pattern {
                    out.push_str("(let-pat ");
                    show_pattern(p, out);
                    out.push(' ');
                    show_expr(value, out);
                    out.push(')');
                    continue;
                }
                out.push_str(if *mutable { "(let-mut " } else { "(let " });
                // `let _` is given an invented name (Ch. 0 §5.2), which is
                // this compiler's business and not the tree's.
                out.push_str(match name.starts_with("#wild") {
                    true => "_",
                    false => name,
                });
                // The written type, when there is one: `let n = 1;` does not
                // say what `n` is and `let n: t9 = 1;` does.
                if let Some(t) = ty {
                    out.push(':');
                    out.push_str(&written_ty(t));
                }
                out.push(' ');
                show_expr(value, out);
                out.push(')');
            }
            Stmt::Expr(e) => {
                out.push_str("(do ");
                show_expr(e, out);
                out.push(')');
            }
        }
    }
    if let Some(t) = &b.tail {
        out.push_str(" (tail ");
        show_expr(t, out);
        out.push(')');
    }
    out.push(')');
}

fn show_pattern(p: &lang::ast::Pattern, out: &mut String) {
    use lang::ast::Pattern;
    match p {
        Pattern::Wild(_) => out.push('_'),
        Pattern::Bind(n, _) => out.push_str(n),
        Pattern::Int(v, _) => out.push_str(&format!("{}", v.to_i128().unwrap_or(0))),
        Pattern::Trit(t, _) => out.push_str(&format!("{}", t.to_i8())),
        Pattern::Char(v, _) => out.push_str(&format!("{v}")),
        Pattern::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        Pattern::Aggregate(path, fields, _) => {
            out.push_str(&format!("(pat {}", path.segments.join("::")));
            for (name, pat) in fields {
                out.push_str(&format!(" ({name} "));
                show_pattern(pat, out);
                out.push(')');
            }
            out.push(')');
        }
        Pattern::Tuple(items, _) => {
            out.push_str("(tuple");
            for i in items {
                out.push(' ');
                show_pattern(i, out);
            }
            out.push(')');
        }
    }
}

fn show_expr(e: &lang::ast::Expr, out: &mut String) {
    use lang::ast::Expr;
    match e {
        Expr::Int(v, _) => out.push_str(&format!("{}", v.to_i128().unwrap_or(0))),
        Expr::Trit(t, _) => out.push_str(&format!("{}", t.to_i8())),
        Expr::Char(v, _) => out.push_str(&format!("{v}")),
        Expr::Path(n, _) => out.push_str(n),
        Expr::Unary(op, a, _) => {
            out.push_str(&format!("({op} "));
            show_expr(a, out);
            out.push(')');
        }
        Expr::Assign(op, a, b, _) | Expr::Binary(op, a, b, _) => {
            out.push_str(&format!("({op} "));
            show_expr(a, out);
            out.push(' ');
            show_expr(b, out);
            out.push(')');
        }
        Expr::Block(b) => show_block(b, out),
        Expr::If(c, then, other, _) => {
            out.push_str("(if ");
            show_expr(c, out);
            out.push(' ');
            show_block(then, out);
            if let Some(o) = other {
                out.push(' ');
                show_expr(o, out);
            }
            out.push(')');
        }
        Expr::For(name, _, iter, b, _) => {
            out.push_str(&format!("(for {name} "));
            show_expr(iter, out);
            out.push(' ');
            show_block(b, out);
            out.push(')');
        }
        Expr::While(c, b, _) => {
            out.push_str("(while ");
            show_expr(c, out);
            out.push(' ');
            show_block(b, out);
            out.push(')');
        }
        Expr::Loop(b, _) => {
            out.push_str("(loop ");
            show_block(b, out);
            out.push(')');
        }
        Expr::Return(v, _) => show_jump("return", v.as_deref(), out),
        Expr::Break(v, _) => show_jump("break", v.as_deref(), out),
        Expr::Continue(_) => out.push_str("(continue)"),
        Expr::Field(base, name, _) => {
            out.push_str("(field ");
            show_expr(base, out);
            out.push_str(&format!(" {name})"));
        }
        Expr::Method(base, name, _, args, _) => {
            out.push_str("(method ");
            show_expr(base, out);
            out.push_str(&format!(" {name}"));
            for a in args {
                out.push(' ');
                show_expr(a, out);
            }
            out.push(')');
        }
        Expr::Call(f, _, _, args, _) => {
            out.push_str(&format!("(call {f}"));
            for a in args {
                out.push(' ');
                show_expr(a, out);
            }
            out.push(')');
        }
        Expr::Unit(_) => out.push_str("(unit)"),
        Expr::Tuple(items, _) => {
            out.push_str("(tuple");
            for i in items {
                out.push(' ');
                show_expr(i, out);
            }
            out.push(')');
        }
        Expr::Match(scrutinee, arms, _) => {
            out.push_str("(match ");
            show_expr(scrutinee, out);
            for arm in arms {
                out.push_str(" (arm (");
                for (i, p) in arm.patterns.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    show_pattern(p, out);
                }
                out.push(')');
                if let Some(g) = &arm.guard {
                    out.push_str(" (when ");
                    show_expr(g, out);
                    out.push(')');
                }
                out.push(' ');
                show_expr(&arm.body, out);
                out.push(')');
            }
            out.push(')');
        }
        Expr::Str(text, _) => {
            out.push_str("(str ");
            for c in text {
                out.push(char::from_u32(*c as u32).unwrap_or('?'));
            }
            out.push(')');
        }
        Expr::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        Expr::Borrow(inner, mutable, _) => {
            out.push_str(if *mutable { "(&mut " } else { "(& " });
            show_expr(inner, out);
            out.push(')');
        }
        Expr::Deref(inner, _) => {
            out.push_str("(* ");
            show_expr(inner, out);
            out.push(')');
        }
        Expr::Index(base, at, _) => {
            out.push_str("(index ");
            show_expr(base, out);
            out.push(' ');
            show_expr(at, out);
            out.push(')');
        }
        Expr::Cast(inner, ty, _) => {
            out.push_str("(as ");
            show_expr(inner, out);
            out.push_str(&format!(" {})", written_ty(ty)));
        }
        Expr::Try(inner, _) => {
            out.push_str("(try ");
            show_expr(inner, out);
            out.push(')');
        }
        Expr::Aggregate(path, fields, _) => {
            out.push_str(&format!("(agg {}", path.segments.join("::")));
            for (name, value) in fields {
                out.push_str(&format!(" ({name} "));
                show_expr(value, out);
                out.push(')');
            }
            out.push(')');
        }
        other => out.push_str(&format!("<{other:?}>")),
    }
}

/// A frontend module to assembly, by the pipeline TIR §6 describes.
fn assemble_module(module: tir::Module, timed: bool) -> Result<String, ExitCode> {
    // A phase timer that costs nothing when it is not asked for. The
    // discipline it enforces is in docs/status.md §10: the two guesses that
    // preceded the last profile were both wrong, and both were about code
    // written the same day.
    let mut at = std::time::Instant::now();
    let mut lap = |name: &str| {
        if timed {
            eprintln!("{name:9} {:>8.1?}", at.elapsed());
        }
        at = std::time::Instant::now();
    };

    lap("frontend");

    let target = tir::TargetDesc::tritium();
    let module = tir::canonicalize_module(&module);
    lap("canon1");
    let mut module = tir::inline_module(&module);
    tir::drop_uncalled(&mut module, &["main"]);
    lap("inline");
    let module = tir::canonicalize_module(&module);
    lap("canon2");

    let legalized = match tir::legalize_module(&module, &target) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("trust: legalization failed: {e:?}");
            return Err(ExitCode::FAILURE);
        }
    };
    lap("legalize");
    // TIR §6's post-condition. The backend is entitled to assume it, so it is
    // checked here rather than trusted, exactly as the tests do.
    let errs = tir::verify_legalized(&legalized, &target);
    if !errs.is_empty() {
        eprintln!("trust: not legalized: {errs:?}");
        return Err(ExitCode::FAILURE);
    }

    lap("verify2");
    let mut asm = match codegen::compile(&legalized, "main") {
        Ok(a) => a,
        Err(e) => {
            eprintln!("trust: code generation failed: {e:?}");
            return Err(ExitCode::FAILURE);
        }
    };
    lap("codegen");
    asm.push_str(RUNTIME);
    Ok(asm)
}

/// Assemble into host memory and execute.
fn run(asm: &str, stats: bool) -> ExitCode {
    let image = match tritium::assemble(asm) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("trust: assembly failed: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let mut vm = tritium::Vm::with_default_memory();
    // The same arrangement `tritium run` uses. Input is read only when the
    // program asks for a code unit, so one that never touches the port does
    // not wait on a stream that may never close; output passes through as it
    // is written, so a program that prints and then faults has printed.
    vm.io =
        tritium::Io::with_source(Box::new(std::io::stdin())).with_sink(Box::new(std::io::stdout()));
    vm.load_image(&image);
    let stop = vm.run(u64::MAX);
    if stats {
        eprintln!("{} instruction(s) retired", vm.steps());
    }
    match stop {
        tritium::Stop::Halted(v) => {
            // A status wider than a shell can carry is reported rather than
            // truncated silently.
            match u8::try_from(v) {
                Ok(c) => ExitCode::from(c),
                Err(_) => {
                    eprintln!("trust: halted with status {v}, which does not fit an exit code");
                    ExitCode::FAILURE
                }
            }
        }
        other => {
            eprintln!("trust: {other}");
            ExitCode::FAILURE
        }
    }
}
