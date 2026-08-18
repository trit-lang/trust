//! The `trustc` command line.

use std::process::ExitCode;
use trit_core::{Bt, Literal, Tint, literal};
use trustc::tir::{self, Val};

const USAGE: &str = "\
trustc — the Trust compiler (draft 0.1)

usage:
    trustc check <file.tir>                    parse and verify a TIR module
    trustc fmt <file.tir>                      print a module in canonical form
    trustc run <file.tir> [@fn] [args…]        interpret a TIR function
    trustc legalize <file.tir> [file.target]   legalize for a target (TIR §6)
    trustc tir <file.tr>                       compile Trust source to TIR
    trustc compile <file.tr|.tir> [@fn]        the whole way to TRISC-27 assembly
    trustc target <file.target>                parse and check a target description

`run` defaults to `@main`; arguments are decimal, `0t` or `0h` literals and
are converted to the callee's parameter widths. `legalize` defaults to the
reference target, \"tritium\".
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "check" => cmd_check(rest),
        "fmt" => cmd_fmt(rest),
        "run" => cmd_run(rest),
        "legalize" => cmd_legalize(rest),
        "tir" => cmd_build(rest),
        "compile" => cmd_compile(rest),
        "target" => cmd_target(rest),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("trustc: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))
}

fn load(path: &str) -> Result<tir::Module, String> {
    let src = read(path)?;
    let module = tir::parse_module(&src).map_err(|e| format!("{path}:{e}"))?;
    let errs = tir::verify(&module);
    if !errs.is_empty() {
        let mut msg = format!("{} verification error(s) in `{path}`:", errs.len());
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        return Err(msg);
    }
    Ok(module)
}

fn cmd_check(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("check: expected a file")?;
    let m = load(path)?;
    println!(
        "ok: {} function(s), {} declaration(s), {} global(s), target \"{}\"",
        m.funcs.len(),
        m.decls.len(),
        m.globals.len(),
        m.target
    );
    Ok(())
}

fn cmd_fmt(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("fmt: expected a file")?;
    let src = read(path)?;
    let module = tir::parse_module(&src).map_err(|e| format!("{path}:{e}"))?;
    print!("{}", tir::print_module(&module));
    Ok(())
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("run: expected a file")?;
    let module = load(path)?;

    let (entry, arg_src) = match args.get(1) {
        Some(a) if a.starts_with('@') => (a[1..].to_string(), &args[2..]),
        _ => ("main".to_string(), &args[1..]),
    };
    let f = module
        .function(&entry)
        .ok_or_else(|| format!("`@{entry}` is not defined in `{path}`"))?;
    if f.sig.params.len() != arg_src.len() {
        return Err(format!(
            "`@{entry}` takes {} argument(s), {} given",
            f.sig.params.len(),
            arg_src.len()
        ));
    }

    let mut vals = Vec::new();
    for ((name, ty), src) in f.sig.params.iter().zip(arg_src) {
        let width = ty
            .width()
            .ok_or_else(|| format!("cannot pass `%{name}: {ty}` from the command line"))?;
        let v: Bt = match literal::parse_literal(src).map_err(|e| e.to_string())? {
            Literal::Int { value, .. } => value,
            Literal::Trit(t) => Bt::from(t),
        };
        let v = Tint::new(width, v.clone())
            .ok_or_else(|| format!("argument `{src}` does not fit in {ty}"))?;
        vals.push(Val::Int(v));
    }

    let mut interp = tir::Interp::new(&module);
    match interp.call(&entry, &vals) {
        Ok(None) => Ok(()),
        Ok(Some(v)) => {
            match &v {
                Val::Int(i) => {
                    println!("{i}  (0t{}, 0h{})", i.to_trit_string(), i.to_hept_string())
                }
                other => println!("{other}"),
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn cmd_legalize(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("legalize: expected a file")?;
    let module = load(path)?;
    let target = match args.get(1) {
        None => tir::TargetDesc::tritium(),
        Some(p) => tir::target::parse_target(&read(p)?).map_err(|e| format!("{p}: {e}"))?,
    };

    let legalized = tir::legalize_module(&module, &target).map_err(|errs| {
        let mut msg = format!(
            "{} instruction(s) could not be legalized for \"{}\":",
            errs.len(),
            target.name
        );
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        msg
    })?;

    // A pass that emits ill-formed IR is worse than one that fails, so the
    // output is re-verified before it is handed on — and against TIR §6's
    // post-condition, not merely well-formedness, since a backend is
    // entitled to assume it.
    let errs = tir::verify_legalized(&legalized, &target);
    if !errs.is_empty() {
        let mut msg = "legalization produced an ill-formed module:".to_string();
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        return Err(msg);
    }
    print!("{}", tir::print_module(&legalized));
    Ok(())
}

/// Compile Trust source to a TIR module, reporting every diagnostic.
fn front_end(path: &str) -> Result<tir::Module, String> {
    let src = read(path)?;
    let module = trustc::lang::compile(&src).map_err(|errs| {
        let mut msg = format!("{} error(s) in `{path}`:", errs.len());
        for e in &errs {
            msg.push_str(&format!("\n  {path}:{e}"));
        }
        msg
    })?;
    let errs = tir::verify(&module);
    if !errs.is_empty() {
        let mut msg = "the frontend produced ill-formed TIR:".to_string();
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        return Err(msg);
    }
    Ok(module)
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("build: expected a file")?;
    print!("{}", tir::print_module(&front_end(path)?));
    Ok(())
}

fn cmd_compile(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("compile: expected a file")?;
    let entry = match args.get(1) {
        Some(a) => a.trim_start_matches('@').to_string(),
        None => "main".to_string(),
    };
    let module = if path.ends_with(".tr") {
        front_end(path)?
    } else {
        load(path)?
    };
    let target = tir::TargetDesc::tritium();

    // TIR §6's pipeline: target-independent optimization, then the mandatory
    // legalization stage, then instruction selection. The backend consumes
    // legalized TIR only, so this runs the pass itself rather than trusting
    // its input.
    // Inlining first, then canonicalize again: splicing a body in exposes
    // constants and slots to the passes that fold and promote them.
    let module = tir::canonicalize_module(&module);
    let mut module = tir::inline_module(&module);
    tir::drop_uncalled(&mut module, &["main"]);
    let module = tir::canonicalize_module(&module);
    let legalized = tir::legalize_module(&module, &target).map_err(|errs| {
        let mut msg = format!("{} instruction(s) could not be legalized:", errs.len());
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        msg
    })?;

    // TIR §6's post-condition, checked at the seam that depends on it: a
    // backend "may assume legalized input and is not required to handle any
    // other", which without this check is a licence to emit anything.
    let errs = tir::verify_legalized(&legalized, &target);
    if !errs.is_empty() {
        let mut msg = "the module reaching code generation is not legalized:".to_string();
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        return Err(msg);
    }

    let asm = trustc::codegen::compile(&legalized, &entry).map_err(|errs| {
        let mut msg = format!("{} code generation error(s):", errs.len());
        for e in &errs {
            msg.push_str(&format!("\n  {e}"));
        }
        msg
    })?;
    print!("{asm}");
    Ok(())
}

fn cmd_target(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("target: expected a file")?;
    let src = read(path)?;
    let desc = tir::target::parse_target(&src).map_err(|e| format!("{path}: {e}"))?;
    println!(
        "ok: target \"{}\": addr_unit={} ptr_width={} word={} legal={:?} call_conv=\"{}\"",
        desc.name, desc.addr_unit, desc.ptr_width, desc.word, desc.legal, desc.call_conv
    );
    Ok(())
}
