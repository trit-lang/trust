//! The `tritium` command line.

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;
use tritium::{Io, Stop, Vm, image};

const USAGE: &str = "\
tritium — the reference TRISC-27 virtual machine (draft 0.1)

usage:
    tritium asm <file.t27> [-o <image>]         assemble source into an image
    tritium run <image> [--mem N] [--steps N]   run an image, stdin to stdout
    tritium dump <image>                        show an image as instructions

An image is a textual list of tryte values (see `tritium::image`), loaded at
address 0. Execution begins there. `asm` writes to stdout without `-o`.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("asm") => cmd_asm(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("dump") => cmd_dump(&args[1..]),
        Some("-h") | Some("--help") | Some("help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
        None => {
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tritium: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read_image(path: &str) -> Result<Vec<i16>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    image::parse(&src).map_err(|e| format!("{path}:{e}"))
}

fn flag(args: &[String], name: &str, default: i128) -> Result<i128, String> {
    match args.iter().position(|a| a == name) {
        None => Ok(default),
        Some(i) => args
            .get(i + 1)
            .ok_or_else(|| format!("{name} needs a value"))?
            .parse::<i128>()
            .map_err(|_| format!("{name} needs a number")),
    }
}

fn cmd_asm(args: &[String]) -> Result<ExitCode, String> {
    let path = args.first().ok_or("asm: expected a source file")?;
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;

    let trytes = tritium::assemble(&src).map_err(|errs| {
        let mut msg = format!("{} assembly error(s) in `{path}`:", errs.len());
        for e in &errs {
            msg.push_str(&format!("\n  {path}:{e}"));
        }
        msg
    })?;

    let text = image::render(&trytes);
    match args.iter().position(|a| a == "-o") {
        Some(i) => {
            let out = args.get(i + 1).ok_or("-o needs a path")?;
            std::fs::write(out, text).map_err(|e| format!("cannot write `{out}`: {e}"))?;
        }
        None => print!("{text}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(args: &[String]) -> Result<ExitCode, String> {
    let path = args.first().ok_or("run: expected an image")?;
    let trytes = read_image(path)?;

    let mem = flag(args, "--mem", tritium::DEFAULT_MEM_SIZE)?;
    let steps = flag(args, "--steps", 100_000_000)? as u64;

    let mut input = Vec::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().read_to_end(&mut input);
    }

    let mut vm = Vm::new(mem);
    vm.io = Io::with_input(&input);
    vm.load_image(&trytes);

    let stop = vm.run(steps);
    let _ = std::io::stdout().write_all(vm.io.output());
    let _ = std::io::stdout().flush();

    match stop {
        Stop::Halted(status) => {
            // A halt status is a word; a process exit code is a byte. Report
            // the low tryte, clamped, and say so when it does not fit.
            let code = u8::try_from(status.rem_euclid(256)).unwrap_or(0);
            if status != 0 {
                eprintln!("tritium: halted with status {status}");
            }
            Ok(ExitCode::from(code))
        }
        other => {
            eprintln!("tritium: {other}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn cmd_dump(args: &[String]) -> Result<ExitCode, String> {
    let path = args.first().ok_or("dump: expected an image")?;
    let trytes = read_image(path)?;
    let mut vm = Vm::new(tritium::DEFAULT_MEM_SIZE);
    vm.load_image(&trytes);

    for i in 0..(trytes.len() as i128 + 2) / 3 {
        let addr = i * 3;
        let word = vm.memory().word(addr);
        match tritium::Inst::decode(word) {
            Ok(inst) => println!("{addr:>8}  {inst}"),
            Err(e) => println!("{addr:>8}  ; {word}  ({e})"),
        }
    }
    Ok(ExitCode::SUCCESS)
}
