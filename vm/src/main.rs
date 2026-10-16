//! The `tritium` command line.

use std::io::Write;
use std::process::ExitCode;
use tritium::{Io, Stop, Vm, image};

const USAGE: &str = "\
tritium — the reference TRISC-27 virtual machine (draft 0.1)

usage:
    tritium asm <file.t27> [-o <image>] [-s <symbols>]   assemble source into an image
    tritium run <image> [--mem N] [--steps N]   run an image, stdin to stdout
    tritium replay <snap> [--steps N] [--watch RANGES] [--core <file>]
                                    resume from a snapshot
    tritium dump <image>                        show an image as instructions
    tritium profile <image> [--mem N] [--steps N]  run it and report what ran

An image is a textual list of tryte values (see `tritium::image`), loaded at
address 0. Execution begins there. `asm` writes to stdout without `-o`;
`-s` writes the symbol table as `address name` lines.

`profile` writes its report to stdout and the program's own output to
stderr, so the two can be separated.

Long-run diagnostics (`run`, environment variables — all off by default):
    TRITIUM_RING=1            record recent PCs, print them on a fault
    TRITIUM_WATCH=RANGES      report stores into inclusive tryte ranges
                              (e.g. \"123\" or \"100-200,300-360\")
    TRITIUM_SNAP_EVERY=N      snapshot every N instructions
    TRITIUM_SNAP_PATH=<file>  also write each snapshot here (see `replay`)
    TRITIUM_CORE_PATH=<file>  on a fault, write the machine here
    TRITIUM_LOG_EVERY=N       print steps and wall time every N instructions
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("asm") => cmd_asm(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
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

    let (trytes, symbols) = tritium::assemble_with_symbols(&src).map_err(|errs| {
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
    if let Some(i) = args.iter().position(|a| a == "-s") {
        let out = args.get(i + 1).ok_or("-s needs a path")?;
        let mut text = String::new();
        for (name, addr) in &symbols {
            text.push_str(&format!("{addr} {name}\n"));
        }
        std::fs::write(out, text).map_err(|e| format!("cannot write `{out}`: {e}"))?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Replay a snapshot (`run` writes them when asked — see USAGE), optionally
/// with a watch armed. The guest's output goes nowhere: a replay exists to
/// observe the machine, not to redo its real work a second time.
fn cmd_replay(args: &[String]) -> Result<ExitCode, String> {
    let path = args.first().ok_or("replay: expected a snapshot file")?;
    let steps = flag(args, "--steps", 100_000_000)? as u64;
    let snap = tritium::Snap::read_file(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    eprintln!(
        "replay: from {} steps, {} pages, pc {}, {} byte(s) of input left",
        snap.steps,
        snap.pages.len(),
        snap.pc,
        snap.io.pending_bytes().len()
    );

    let mut vm = Vm::from_snap(&snap);
    vm.io = vm.io.with_sink(Box::new(std::io::sink()));
    vm.enable_ring(4096);
    if let Some(i) = args.iter().position(|a| a == "--watch") {
        let spec = args.get(i + 1).ok_or("--watch needs ranges")?;
        vm.set_watch(parse_watch(spec)?);
    }

    let stop = run_logged(&mut vm, steps);
    if let Some(i) = args.iter().position(|a| a == "--core") {
        let out = args.get(i + 1).ok_or("--core needs a path")?;
        vm.capture_snap()
            .write_file(out)
            .map_err(|e| format!("cannot write core `{out}`: {e}"))?;
    }
    match stop {
        Stop::Halted(status) => {
            eprintln!("replay: halted with status {status} after {} steps", vm.steps());
            Ok(ExitCode::SUCCESS)
        }
        Stop::OutOfSteps => {
            eprintln!("replay: step budget reached after {} steps", vm.steps());
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("{}", vm.report(&other));
            Ok(ExitCode::FAILURE)
        }
    }
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
    // Snapshots need the input reproducible and re-readable, so a run that
    // may snapshot (or record a ring for replay) reads all of stdin up
    // front and closes it. A plain run keeps lazy streaming.
    let diag = std::env::var_os("TRITIUM_SNAP_EVERY").is_some()
        || std::env::var_os("TRITIUM_WATCH").is_some()
        || std::env::var_os("TRITIUM_RING").is_some();
    if diag {
        let mut input = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut input)
            .map_err(|e| format!("cannot read stdin: {e}"))?;
        vm.io = Io::with_input(&input).with_sink(Box::new(std::io::stdout()));
    } else {
        vm.io = Io::with_source(Box::new(std::io::stdin())).with_sink(Box::new(std::io::stdout()));
    }
    arm_diag(&mut vm);
    vm.load_image(&trytes);

    let stop = run_logged(&mut vm, steps);
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
            eprintln!("{}", vm.report(&other));
            if let Some(path) = std::env::var_os("TRITIUM_CORE_PATH") {
                let path = path.to_string_lossy().into_owned();
                match vm.capture_snap().write_file(&path) {
                    Ok(()) => eprintln!("tritium: core written to {path}"),
                    Err(e) => eprintln!("tritium: cannot write core to {path}: {e}"),
                }
            }
            if let Some(snap) = vm.last_snap() {
                eprintln!("tritium: last snapshot at {} steps", snap.steps);
            }
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Turn on whatever diagnostics the environment asks for.
fn arm_diag(vm: &mut Vm) {
    if std::env::var_os("TRITIUM_RING").is_some() {
        vm.enable_ring(4096);
    }
    if let Some(spec) = std::env::var_os("TRITIUM_WATCH") {
        match parse_watch(&spec.to_string_lossy()) {
            Ok(ranges) => vm.set_watch(ranges),
            Err(e) => eprintln!("tritium: bad TRITIUM_WATCH: {e}"),
        }
    }
    if let Some(n) = std::env::var_os("TRITIUM_SNAP_EVERY")
        .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
    {
        let path = std::env::var_os("TRITIUM_SNAP_PATH").map(|p| p.to_string_lossy().into_owned());
        vm.enable_snapshots(n, path);
    }
}

/// Parse `123,100-200` into inclusive tryte ranges.
fn parse_watch(spec: &str) -> Result<Vec<(i128, i128)>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let parse = |s: &str| s.parse::<i128>().map_err(|_| format!("`{s}` is not a number"));
        let range = match part.split_once('-') {
            Some((a, b)) => (parse(a)?, parse(b)?),
            None => (parse(part)?, parse(part)?),
        };
        if range.0 > range.1 {
            return Err(format!("`{part}` is backwards"));
        }
        out.push(range);
    }
    if out.is_empty() {
        return Err("no ranges given".to_string());
    }
    Ok(out)
}

/// `run`, but with a progress line every `TRITIUM_LOG_EVERY` instructions.
fn run_logged(vm: &mut Vm, steps: u64) -> Stop {
    let every = std::env::var_os("TRITIUM_LOG_EVERY")
        .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
        .unwrap_or(0);
    if every == 0 {
        return vm.run(steps);
    }
    let start = std::time::Instant::now();
    let mut left = steps;
    while left > 0 {
        let n = left.min(every);
        let stop = vm.run(n);
        if !matches!(stop, Stop::OutOfSteps) {
            return stop;
        }
        left -= n;
        eprintln!("[log] steps={} wall={:?}", vm.steps(), start.elapsed());
    }
    Stop::OutOfSteps
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
