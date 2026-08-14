//! The `tritium` command line.

use std::io::Write;
use std::process::ExitCode;
use tritium::{Io, Stop, Vm, image};

const USAGE: &str = "\
tritium — the reference TRISC-27 virtual machine (draft 0.1)

usage:
    tritium asm <file.t27> [-o <image>]         assemble source into an image
    tritium run <image> [--mem N] [--steps N]   run an image, stdin to stdout
    tritium dump <image>                        show an image as instructions
    tritium profile <image> [--mem N] [--steps N]  run it and report what ran

An image is a textual list of tryte values (see `tritium::image`), loaded at
address 0. Execution begins there. `asm` writes to stdout without `-o`.

`profile` writes its report to stdout and the program's own output to
stderr, so the two can be separated.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("asm") => cmd_asm(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("dump") => cmd_dump(&args[1..]),
        Some("profile") => cmd_profile(&args[1..]),
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

    // Input is read only when the program asks for a code unit, so a program
    // that never touches the port does not wait on a stream that may never
    // close. Output passes through as it is written.
    let mut vm = Vm::new(mem);
    vm.io = Io::with_source(Box::new(std::io::stdin())).with_sink(Box::new(std::io::stdout()));
    vm.load_image(&trytes);

    let stop = vm.run(steps);
    let _ = std::io::stdout().flush();

    match stop {
        Stop::Halted(status) => {
            // A halt status is a word; a process exit code is a byte. Report
            // the low tryte, clamped, and say so when it does not fit.
            let code = u8::try_from(status.rem_euclid(256)).unwrap_or(0);
            if status != 0 {
                eprintln!(
                    "tritium: halted with status {status} after {} instruction(s)",
                    vm.steps()
                );
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

/// Run an image one instruction at a time, counting what ran.
///
/// The machine has a cycle counter (TRISC-27 §2.3), which answers "how many"
/// but never "which". This answers "which", and it exists because the answer
/// was surprising: the change that looked obviously most valuable by reading
/// the code turned out to be fourth by measurement (`docs/spec-gaps.md`
/// G8.6).
fn cmd_profile(args: &[String]) -> Result<ExitCode, String> {
    let path = args.first().ok_or("profile: expected an image")?;
    let trytes = read_image(path)?;
    let mem = flag(args, "--mem", tritium::DEFAULT_MEM_SIZE)?;
    let cap = flag(args, "--steps", 100_000_000)? as u64;

    let mut vm = Vm::new(mem);
    // The report is this command's output; the program's belongs elsewhere.
    vm.io = Io::with_source(Box::new(std::io::stdin())).with_sink(Box::new(std::io::stderr()));
    vm.load_image(&trytes);

    let p = tritium::profile(&mut vm, cap);
    let _ = std::io::stderr().flush();

    match &p.stop {
        Some(s) => println!("{s}"),
        None => println!("step cap of {cap} reached"),
    }
    println!("{} instruction(s) retired\n", p.total);

    let mut kinds: Vec<_> = p.by_kind.iter().collect();
    kinds.sort_by_key(|(k, n)| (std::cmp::Reverse(**n), (*k).clone()));
    for (k, n) in &kinds {
        println!("{k:<16} {n:>12}  {:>6.2}%", p.share(**n));
    }

    // What the frame costs, which is the question a compiler writer brings.
    let (frame, data) = (p.frame_traffic(), p.data_traffic());
    println!(
        "\nframe traffic    {frame:>12}  {:>6.2}%\ndata traffic     {data:>12}  {:>6.2}%",
        p.share(frame),
        p.share(data)
    );

    // Where the time is. A steep curve says the work to do is small and
    // findable; a flat one says there is no hot loop to find.
    let hot = p.hottest();
    println!("\n{} distinct word(s) executed", hot.len());
    for n in [20usize, 100, 400] {
        let acc: u64 = hot.iter().take(n).map(|(_, c)| c).sum();
        println!("  hottest {n:>4}  {:>6.2}%", p.share(acc));
    }

    println!("\naddress        executions");
    for (a, n) in hot.iter().take(20) {
        println!("{a:>8} {n:>16}  {:>6.2}%", p.share(*n));
    }
    Ok(ExitCode::SUCCESS)
}
