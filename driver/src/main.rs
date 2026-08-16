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
    trust run <file.tr> [--stats]      compile, link the runtime, execute
    trust asm <file.tr>                print the assembly it would run

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

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("trust: cannot read `{path}`: {e}");
            return ExitCode::FAILURE;
        }
    };

    let asm = match assemble_source(&src, path) {
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

/// Source to assembly, by the pipeline TIR §6 describes.
fn assemble_source(src: &str, path: &str) -> Result<String, ExitCode> {
    let module = match lang::compile(src) {
        Ok(m) => m,
        Err(errs) => {
            eprintln!("trust: {} error(s) in `{path}`:", errs.len());
            for e in &errs {
                eprintln!("  {path}:line {}: {}", e.line, e.message);
            }
            return Err(ExitCode::FAILURE);
        }
    };
    let errs = tir::verify(&module);
    if !errs.is_empty() {
        eprintln!("trust: the frontend emitted ill-formed TIR: {errs:?}");
        return Err(ExitCode::FAILURE);
    }

    let target = tir::TargetDesc::tritium();
    let module = tir::canonicalize_module(&module);
    let mut module = tir::inline_module(&module);
    tir::drop_uncalled(&mut module, &["main"]);
    let module = tir::canonicalize_module(&module);

    let legalized = match tir::legalize_module(&module, &target) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("trust: legalization failed: {e:?}");
            return Err(ExitCode::FAILURE);
        }
    };
    // TIR §6's post-condition. The backend is entitled to assume it, so it is
    // checked here rather than trusted, exactly as the tests do.
    let errs = tir::verify_legalized(&legalized, &target);
    if !errs.is_empty() {
        eprintln!("trust: not legalized: {errs:?}");
        return Err(ExitCode::FAILURE);
    }

    let mut asm = match codegen::compile(&legalized, "main") {
        Ok(a) => a,
        Err(e) => {
            eprintln!("trust: code generation failed: {e:?}");
            return Err(ExitCode::FAILURE);
        }
    };
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
    vm.load_image(&image);
    let stop = vm.run(u64::MAX);
    // The program's own output first, whatever happened: a program that
    // printed and then faulted printed.
    print!("{}", String::from_utf8_lossy(vm.io.output()));
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
