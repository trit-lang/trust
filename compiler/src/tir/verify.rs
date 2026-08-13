//! The TIR verifier: well-formedness, SSA dominance, and type checking.
//!
//! Every rule here comes from TIR §1–§3. A module that passes the verifier is
//! one the interpreter and any backend may assume is structurally sound;
//! passes are expected to re-verify their output in debug builds.

use super::ir::*;
use std::collections::{BTreeMap, BTreeSet};
use trit_core::{Flavor, MAX_WIDTH};

/// One verification failure, located by function and block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerifyError {
    /// The function the error is in, if any.
    pub function: Option<String>,
    /// The block the error is in, if any.
    pub block: Option<String>,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.function, &self.block) {
            (Some(fun), Some(b)) => write!(f, "@{fun} ^{b}: {}", self.message),
            (Some(fun), None) => write!(f, "@{fun}: {}", self.message),
            _ => write!(f, "{}", self.message),
        }
    }
}

/// Verify a module. An empty result means it is well-formed.
pub fn verify(m: &Module) -> Vec<VerifyError> {
    let mut errs = Vec::new();

    if m.version != TIR_VERSION {
        errs.push(VerifyError {
            function: None,
            block: None,
            message: format!(
                "module declares TIR version `{}`, not `{TIR_VERSION}`",
                m.version
            ),
        });
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for name in m
        .globals
        .iter()
        .map(|g| g.name.as_str())
        .chain(m.funcs.iter().map(|f| f.sig.name.as_str()))
        .chain(m.decls.iter().map(|d| d.name.as_str()))
    {
        if !seen.insert(name) {
            errs.push(VerifyError {
                function: None,
                block: None,
                message: format!("`@{name}` is defined more than once"),
            });
        }
    }

    for g in &m.globals {
        let Some(init) = &g.init else { continue };
        let mut at = 0u32;
        for item in init {
            match item {
                InitItem::Tryte(v) if !v.fits_width(9) => errs.push(VerifyError {
                    function: None,
                    block: None,
                    message: format!(
                        "`@{}` initializer tryte {at} is {v}, which does not fit in one tryte",
                        g.name
                    ),
                }),
                // A relocation must name something this module has (§1.2).
                InitItem::Addr(name)
                    if m.signature(name).is_none()
                        && !m.globals.iter().any(|g| &g.name == name) =>
                {
                    errs.push(VerifyError {
                        function: None,
                        block: None,
                        message: format!(
                            "`@{}` takes the address of `@{name}`, which this module does \
                             not define or declare",
                            g.name
                        ),
                    });
                }
                _ => {}
            }
            at += item.trytes();
        }
        if at != g.trytes {
            errs.push(VerifyError {
                function: None,
                block: None,
                message: format!(
                    "`@{}` initializer fills {at} trytes but the global is tryte[{}]",
                    g.name, g.trytes
                ),
            });
        }
    }

    for f in &m.funcs {
        verify_function(m, f, &mut errs);
    }
    errs
}

struct Ctx<'a> {
    module: &'a Module,
    func: &'a Function,
    errs: &'a mut Vec<VerifyError>,
    block: String,
    types: BTreeMap<String, Type>,
    live: BTreeSet<String>,
}

impl Ctx<'_> {
    fn err(&mut self, message: impl Into<String>) {
        self.errs.push(VerifyError {
            function: Some(self.func.sig.name.clone()),
            block: Some(self.block.clone()),
            message: message.into(),
        });
    }

    /// The type of an operand, reporting undefined or out-of-scope uses.
    fn operand_type(&mut self, o: &Operand) -> Option<Type> {
        match o {
            Operand::Const(t, v) => {
                if let Type::Int(n) = t
                    && !v.fits_width(*n)
                {
                    self.err(format!("constant {v} does not fit in {t}"));
                }
                Some(*t)
            }
            Operand::Global(name) => {
                if !self.module.globals.iter().any(|g| &g.name == name) {
                    self.err(format!("`@{name}` is not a global in this module"));
                }
                Some(Type::Ptr)
            }
            Operand::Value(name) => match self.types.get(name) {
                None => {
                    self.err(format!("`%{name}` is not defined in this function"));
                    None
                }
                Some(t) => {
                    let t = *t;
                    if !self.live.contains(name) {
                        // SSA: every use must be dominated by its definition.
                        self.err(format!(
                            "`%{name}` is used here but its definition does not dominate this block"
                        ));
                    }
                    Some(t)
                }
            },
        }
    }

    fn expect(&mut self, o: &Operand, want: Type, what: &str) {
        if let Some(got) = self.operand_type(o)
            && got != want
        {
            self.err(format!("{what} has type {got}, expected {want}"));
        }
    }
}

fn check_width(ty: Type, ctx: &mut Ctx) {
    if let Type::Int(n) = ty
        && n > MAX_WIDTH
    {
        ctx.err(format!(
            "width t{n} exceeds the module maximum of t{MAX_WIDTH} (TIR §2)"
        ));
    }
}

fn verify_function(m: &Module, f: &Function, errs: &mut Vec<VerifyError>) {
    // Block labels must be unique, and the entry block may not be a branch
    // target (TIR §1.1).
    let mut labels: BTreeSet<&str> = BTreeSet::new();
    for b in &f.blocks {
        if !labels.insert(&b.label) {
            errs.push(VerifyError {
                function: Some(f.sig.name.clone()),
                block: Some(b.label.clone()),
                message: "duplicate block label".into(),
            });
        }
    }
    let entry = &f.blocks[0];
    if !entry.params.is_empty() {
        errs.push(VerifyError {
            function: Some(f.sig.name.clone()),
            block: Some(entry.label.clone()),
            message: "the entry block takes the function's parameters and may not declare its own"
                .into(),
        });
    }

    // Collect every definition and its type before checking uses, so that a
    // forward reference reports "does not dominate" rather than "undefined".
    let mut types: BTreeMap<String, Type> = BTreeMap::new();
    let mut defs: BTreeMap<&str, Vec<(String, Type)>> = BTreeMap::new();
    let mut redefined: Vec<String> = Vec::new();
    let mut define = |types: &mut BTreeMap<String, Type>, name: &str, ty: Type| {
        if types.insert(name.to_string(), ty).is_some() {
            redefined.push(name.to_string());
        }
    };
    for (n, t) in &f.sig.params {
        define(&mut types, n, *t);
    }
    for b in &f.blocks {
        let mut block_defs = Vec::new();
        for (n, t) in &b.params {
            define(&mut types, n, *t);
            block_defs.push((n.clone(), *t));
        }
        for inst in &b.insts {
            for (n, t) in result_types(m, inst) {
                define(&mut types, &n, t);
                block_defs.push((n, t));
            }
        }
        defs.insert(&b.label, block_defs);
    }
    for name in redefined {
        errs.push(VerifyError {
            function: Some(f.sig.name.clone()),
            block: None,
            message: format!("`%{name}` is defined more than once (SSA)"),
        });
    }

    let doms = dominators(f);
    let reachable: BTreeSet<&str> = doms.keys().copied().collect();
    if reachable.len() != f.blocks.len() {
        for b in &f.blocks {
            if !reachable.contains(b.label.as_str()) {
                errs.push(VerifyError {
                    function: Some(f.sig.name.clone()),
                    block: Some(b.label.clone()),
                    message: "block is unreachable from the entry block".into(),
                });
            }
        }
    }
    for b in &f.blocks {
        for succ in successors(&b.term) {
            if succ.label == entry.label {
                errs.push(VerifyError {
                    function: Some(f.sig.name.clone()),
                    block: Some(b.label.clone()),
                    message: "the entry block may not be a branch target (TIR §1.1)".into(),
                });
            }
        }
    }

    for b in &f.blocks {
        // Values in scope: this block's own definitions, plus everything
        // defined by blocks that strictly dominate it, plus the function
        // parameters.
        let mut live: BTreeSet<String> = f.sig.params.iter().map(|(n, _)| n.clone()).collect();
        if let Some(dominating) = doms.get(b.label.as_str()) {
            for d in dominating {
                if *d == b.label {
                    continue;
                }
                for (n, _) in defs.get(d).into_iter().flatten() {
                    live.insert(n.clone());
                }
            }
        }
        for (n, _) in &b.params {
            live.insert(n.clone());
        }

        let mut ctx = Ctx {
            module: m,
            func: f,
            errs,
            block: b.label.clone(),
            types: types.clone(),
            live,
        };

        for inst in &b.insts {
            verify_inst(inst, &mut ctx);
            for (n, _) in result_types(m, inst) {
                ctx.live.insert(n);
            }
        }
        verify_terminator(&b.term, f, &mut ctx);
    }
}

/// The types an instruction defines, in result order.
fn result_types(m: &Module, inst: &Inst) -> Vec<(String, Type)> {
    let tys: Vec<Type> = match &inst.kind {
        InstKind::Flavored { flavor, ty, .. } => {
            if *flavor == Flavor::Flag {
                vec![*ty, Type::Int(1)]
            } else {
                vec![*ty]
            }
        }
        InstKind::Plain { ty, .. } => vec![*ty],
        InstKind::Neg { ty, .. } => vec![*ty],
        InstKind::Cmp { .. } => vec![Type::Int(1)],
        InstKind::Select3 { ty, .. } => vec![*ty],
        InstKind::Slot { .. } | InstKind::Offset { .. } => vec![Type::Ptr],
        InstKind::Load { ty, .. } => vec![*ty],
        InstKind::Store { .. } => vec![],
        InstKind::Widen { to, .. } | InstKind::Trunc { to, .. } => vec![*to],
        InstKind::Call { callee, ret, .. } => {
            // Trust the module's signature when there is one; the call's own
            // annotation is checked against it in verify_inst.
            match callee_signature(m, callee).map(|s| s.ret).unwrap_or(*ret) {
                Some(t) => vec![t],
                None => vec![],
            }
        }
    };
    inst.results.iter().cloned().zip(tys).collect()
}

/// The signature a call is checked against: the callee's, when the callee is
/// a symbol. An indirect call has none — TIR §3.7 makes the call site's own
/// types the signature, and a mismatch UB rather than a diagnosable error.
fn callee_signature<'a>(m: &'a Module, callee: &Callee) -> Option<&'a Signature> {
    match callee {
        Callee::Direct(name) => m.signature(name),
        Callee::Indirect(_) => None,
    }
}

fn verify_inst(inst: &Inst, ctx: &mut Ctx) {
    let expected_results = match &inst.kind {
        InstKind::Flavored { flavor, .. } if *flavor == Flavor::Flag => 2,
        InstKind::Store { .. } => 0,
        InstKind::Call { callee, ret, .. } => {
            let declared = callee_signature(ctx.module, callee)
                .map(|s| s.ret)
                .unwrap_or(*ret);
            usize::from(declared.is_some())
        }
        _ => 1,
    };
    if inst.results.len() != expected_results {
        ctx.err(format!(
            "instruction defines {} results, expected {expected_results}",
            inst.results.len()
        ));
    }

    match &inst.kind {
        InstKind::Flavored { op, ty, a, b, .. } => {
            check_int(*ty, ctx, op.name());
            check_width(*ty, ctx);
            ctx.expect(a, *ty, "left operand");
            ctx.expect(b, *ty, "right operand");
        }
        InstKind::Plain { op, ty, a, b } => {
            check_int(*ty, ctx, op.name());
            check_width(*ty, ctx);
            ctx.expect(a, *ty, "left operand");
            ctx.expect(b, *ty, "right operand");
        }
        InstKind::Neg { ty, a } => {
            check_int(*ty, ctx, "neg");
            ctx.expect(a, *ty, "operand");
        }
        InstKind::Cmp { ty, a, b } => {
            check_int(*ty, ctx, "cmp");
            ctx.expect(a, *ty, "left operand");
            ctx.expect(b, *ty, "right operand");
        }
        InstKind::Select3 {
            t,
            ty,
            neg,
            zero,
            pos,
        } => {
            ctx.expect(t, Type::Int(1), "select3 selector");
            for (o, what) in [(neg, "−1 arm"), (zero, "0 arm"), (pos, "+1 arm")] {
                ctx.expect(o, *ty, what);
            }
        }
        InstKind::Slot { trytes } => {
            if *trytes == 0 {
                ctx.err("`slot tryte[0]` allocates nothing");
            }
        }
        // A `ptr` may be loaded and stored as itself. That is not the
        // integer↔pointer conversion TIR §5 declines to define: provenance
        // travels with the value, and nothing about an address is exposed.
        // Without it a reference cannot be spilled, a struct cannot hold one,
        // and a fat pointer cannot exist (`docs/spec-gaps.md` G6.7).
        InstKind::Load { ty, p } => {
            check_storable(*ty, ctx, "load");
            ctx.expect(p, Type::Ptr, "load address");
        }
        InstKind::Store { ty, v, p } => {
            check_storable(*ty, ctx, "store");
            ctx.expect(v, *ty, "stored value");
            ctx.expect(p, Type::Ptr, "store address");
        }
        InstKind::Offset { p, d } => {
            ctx.expect(p, Type::Ptr, "offset base");
            if let Some(t) = ctx.operand_type(d)
                && t.width().is_none()
            {
                ctx.err(format!(
                    "offset displacement has type {t}, expected an integer"
                ));
            }
        }
        InstKind::Widen { from, a, to } => {
            ctx.expect(a, *from, "operand");
            match (from.width(), to.width()) {
                (Some(m), Some(n)) if m >= n => {
                    ctx.err(format!("`widen {from} -> {to}` does not widen"));
                }
                (Some(_), Some(_)) => check_width(*to, ctx),
                _ => ctx.err("`widen` operates on integers"),
            }
        }
        InstKind::Trunc { from, a, to } => {
            ctx.expect(a, *from, "operand");
            match (from.width(), to.width()) {
                (Some(m), Some(n)) if n >= m => {
                    ctx.err(format!("`trunc {from} -> {to}` does not narrow"));
                }
                (Some(_), Some(_)) => {}
                _ => ctx.err("`trunc` operates on integers"),
            }
        }
        InstKind::Call { callee, args, ret } => {
            let name = match callee {
                Callee::Direct(n) => n.clone(),
                // Indirect: TIR §3.7 makes the call site's own types the
                // signature, so there is nothing here to check them against.
                // The pointer itself must be one.
                Callee::Indirect(p) => {
                    ctx.expect(p, Type::Ptr, "callee");
                    return;
                }
            };
            match ctx.module.signature(&name) {
                None => ctx.err(format!("`@{name}` is neither declared nor defined")),
                Some(sig) => {
                    let sig = sig.clone();
                    if sig.params.len() != args.len() {
                        ctx.err(format!(
                            "`@{name}` takes {} arguments, {} given",
                            sig.params.len(),
                            args.len()
                        ));
                    }
                    for (arg, (_, want)) in args.iter().zip(&sig.params) {
                        ctx.expect(arg, *want, "argument");
                    }
                    if *ret != sig.ret {
                        ctx.err(format!(
                            "call annotates `@{name}` as returning {}, but it returns {}",
                            show_ret(*ret),
                            show_ret(sig.ret)
                        ));
                    }
                }
            }
        }
    }
}

fn show_ret(t: Option<Type>) -> String {
    match t {
        Some(t) => t.to_string(),
        None => "()".to_string(),
    }
}

fn check_int(ty: Type, ctx: &mut Ctx, what: &str) {
    if ty.width().is_none() {
        ctx.err(format!("`{what}` operates on integers, not {ty}"));
    }
}

/// Memory accesses take an integer width or `ptr`.
fn check_storable(_ty: Type, _ctx: &mut Ctx, _what: &str) {}

fn verify_terminator(t: &Terminator, f: &Function, ctx: &mut Ctx) {
    match t {
        Terminator::Br3 { t, neg, zero, pos } => {
            ctx.expect(t, Type::Int(1), "branch selector");
            for target in [neg, zero, pos] {
                verify_target(target, f, ctx);
            }
        }
        Terminator::Br(d) => verify_target(d, f, ctx),
        Terminator::Ret(v) => match (v, f.sig.ret) {
            (Some(v), Some(want)) => ctx.expect(v, want, "returned value"),
            (None, None) => {}
            (Some(_), None) => ctx.err("returning a value from a `()` function"),
            (None, Some(want)) => ctx.err(format!("`ret` with no value, expected {want}")),
        },
        Terminator::Trap(_) | Terminator::Unreachable => {}
    }
}

fn verify_target(target: &Target, f: &Function, ctx: &mut Ctx) {
    let Some(dest) = f.block(&target.label) else {
        ctx.err(format!(
            "`^{}` is not a block in this function",
            target.label
        ));
        return;
    };
    if dest.params.len() != target.args.len() {
        ctx.err(format!(
            "`^{}` takes {} block parameters, {} arguments given",
            dest.label,
            dest.params.len(),
            target.args.len()
        ));
        return;
    }
    for (arg, (name, want)) in target.args.iter().zip(&dest.params) {
        ctx.expect(arg, *want, &format!("argument for `%{name}`"));
    }
}

/// The blocks a terminator may transfer control to.
pub fn successors(t: &Terminator) -> Vec<&Target> {
    match t {
        Terminator::Br3 { neg, zero, pos, .. } => vec![neg, zero, pos],
        Terminator::Br(d) => vec![d],
        Terminator::Ret(_) | Terminator::Trap(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// The blocks that can branch to `label`.
pub fn predecessors<'a>(f: &'a Function, label: &str) -> Vec<&'a Block> {
    f.blocks
        .iter()
        .filter(|b| successors(&b.term).iter().any(|t| t.label == label))
        .collect()
}

/// For each reachable block, the set of blocks that dominate it (including
/// itself). Iterative fixpoint over the intersection rule — the textbook
/// algorithm, which is ample for the block counts a frontend emits.
pub fn dominators(f: &Function) -> BTreeMap<&str, BTreeSet<&str>> {
    let entry = f.blocks[0].label.as_str();
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![entry];
    while let Some(l) = stack.pop() {
        if !reachable.insert(l) {
            continue;
        }
        if let Some(b) = f.block(l) {
            for s in successors(&b.term) {
                if f.block(&s.label).is_some() {
                    stack.push(&s.label);
                }
            }
        }
    }

    let all: BTreeSet<&str> = reachable.iter().copied().collect();
    let mut dom: BTreeMap<&str, BTreeSet<&str>> = reachable
        .iter()
        .map(|&l| {
            (
                l,
                if l == entry {
                    BTreeSet::from([entry])
                } else {
                    all.clone()
                },
            )
        })
        .collect();

    let mut changed = true;
    while changed {
        changed = false;
        for &l in &reachable {
            if l == entry {
                continue;
            }
            let preds: Vec<&str> = predecessors(f, l)
                .iter()
                .map(|b| b.label.as_str())
                .filter(|p| reachable.contains(p))
                .collect();
            let mut new: BTreeSet<&str> = match preds.split_first() {
                None => BTreeSet::new(),
                Some((first, rest)) => rest.iter().fold(dom[first].clone(), |acc, p| {
                    acc.intersection(&dom[p]).copied().collect()
                }),
            };
            new.insert(l);
            if new != dom[l] {
                dom.insert(l, new);
                changed = true;
            }
        }
    }
    dom
}
