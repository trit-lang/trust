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
