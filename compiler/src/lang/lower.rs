//! Type checking and lowering to TIR (Language Ch. 0, Ch. 1).
//!
//! One pass does both: an expression is checked and emitted together, with
//! the expected type passed down so that an integer literal takes its type
//! from context (Ch. 1 §3) rather than needing inference machinery.
//!
//! # Every local lives in a slot
//!
//! TIR is SSA, and a mutable local is not. Rather than construct SSA in the
//! frontend, every local gets a `slot`: a read is a `load`, a write is a
//! `store`. Control flow then needs no block parameters at all, because no
//! value crosses a block edge in a register — which is why this file has no
//! phi placement in it. The cost is real and is paid back by the optimizer
//! that does not exist yet; the benefit is that this pass is small enough to
//! be obviously right.
//!
//! Expressions whose value is produced in more than one block — `if`, `match`,
//! a `loop` with `break`-with-value — land in a temporary slot for the same
//! reason.

use super::ast;
use super::lex::{Line, SyntaxError};
use crate::tir::ir::{self, *};
use std::collections::HashMap;
use trit_core::{Bt, FaultCode, Flavor};

/// A type-checking error.
pub type Error = SyntaxError;

type R<T> = Result<T, Error>;

fn err<T>(line: Line, message: impl Into<String>) -> R<T> {
    Err(SyntaxError {
        line,
        message: message.into(),
    })
}

/// The types this milestone knows (Ch. 1 §2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// `trit`.
    Trit,
    /// `bool` — a distinct nominal type, not a subrange of `trit`.
    Bool,
    /// `t9`.
    T9,
    /// `t27`.
    T27,
    /// `taddr` — distinct from `t27` even where it is the same width, so that
    /// mixing them needs an explicit `as` (Ch. 1, P2).
    TAddr,
    /// `()`.
    Unit,
    /// `[T; N]`.
    Array(Box<Ty>, i128),
    /// The type of an expression that never produces a value: `break`,
    /// `continue`, `return`.
    Never,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Trit => f.write_str("trit"),
            Ty::Bool => f.write_str("bool"),
            Ty::T9 => f.write_str("t9"),
            Ty::T27 => f.write_str("t27"),
            Ty::TAddr => f.write_str("taddr"),
            Ty::Unit => f.write_str("()"),
            Ty::Array(t, n) => write!(f, "[{t}; {n}]"),
            Ty::Never => f.write_str("!"),
        }
    }
}

impl Ty {
    /// The TIR type a value of this type is held in.
    fn tir(&self) -> Type {
        match self {
            Ty::Trit | Ty::Bool => Type::Int(1),
            Ty::T9 => Type::Int(9),
            Ty::T27 | Ty::TAddr | Ty::Never | Ty::Unit => Type::Int(27),
            Ty::Array(..) => Type::Ptr,
        }
    }

    /// Width in trits, for the numeric types.
    fn width(&self) -> Option<u32> {
        match self {
            Ty::Trit | Ty::Bool => Some(1),
            Ty::T9 => Some(9),
            Ty::T27 | Ty::TAddr => Some(27),
            _ => None,
        }
    }

    /// True for the types arithmetic operates on (Ch. 1 §4).
    fn is_arithmetic(&self) -> bool {
        matches!(self, Ty::T9 | Ty::T27 | Ty::TAddr)
    }

    /// Size in trytes (Ch. 1 §7).
    fn size(&self) -> i128 {
        match self {
            Ty::Trit | Ty::Bool | Ty::T9 => 1,
            Ty::T27 | Ty::TAddr => 3,
            Ty::Unit | Ty::Never => 0,
            Ty::Array(t, n) => t.size() * n,
        }
    }

    /// Whether a value of this type is held in a register or in memory.
    fn is_scalar(&self) -> bool {
        !matches!(self, Ty::Array(..) | Ty::Unit | Ty::Never)
    }
}

/// A local binding.
#[derive(Clone)]
struct Local {
    /// The SSA value holding its slot's address.
    slot: String,
    ty: Ty,
    mutable: bool,
}

/// What a name at item scope refers to.
enum Global {
    /// A scalar constant, inlined at every use.
    Const(Bt, Ty),
    /// An array constant, which lives in a TIR global.
    Array(String, Ty),
}

/// Lower a parsed file into a TIR module.
pub fn lower(file: &ast::File) -> Result<Module, Vec<Error>> {
    let mut errs = Vec::new();
    let mut module = Module {
        version: TIR_VERSION.to_string(),
        target: "tritium".to_string(),
        ..Module::default()
    };

    // Signatures first: every item in the file is visible to every other,
    // whatever the order (Ch. 0 §3).
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    for item in &file.items {
        if let ast::Item::Fn(f) = item {
            let params: Result<Vec<Ty>, Error> =
                f.params.iter().map(|(_, t)| resolve_ty(t)).collect();
            let ret = match &f.ret {
                None => Ok(Ty::Unit),
                Some(t) => resolve_ty(t),
            };
            match (params, ret) {
                (Ok(p), Ok(r)) => {
                    if sigs.insert(f.name.clone(), (p, r)).is_some() {
                        errs.push(SyntaxError {
                            line: f.line,
                            message: format!("`{}` is defined more than once", f.name),
                        });
                    }
                }
                (Err(e), _) | (_, Err(e)) => errs.push(e),
            }
        }
    }

    // Constants next, since a function body may use one.
    let mut globals: HashMap<String, Global> = HashMap::new();
    for item in &file.items {
        if let ast::Item::Const(c) = item {
            match const_item(c, &mut module) {
                Ok(g) => {
                    if globals.insert(c.name.clone(), g).is_some() {
                        errs.push(SyntaxError {
                            line: c.line,
                            message: format!("`{}` is defined more than once", c.name),
                        });
                    }
                }
                Err(e) => errs.push(e),
            }
        }
    }

    for item in &file.items {
        let ast::Item::Fn(f) = item else { continue };
        let signature = signature_of(f, &sigs);
        match &f.body {
            // A function without a body is a declaration, and lowers to TIR's
            // own declaration form — one mechanism, spelled twice.
            None => module.decls.push(signature),
            Some(body) => match function(f, signature, body, &sigs, &globals) {
                Ok(func) => module.funcs.push(func),
                Err(e) => errs.push(e),
            },
        }
    }

    if errs.is_empty() {
        Ok(module)
    } else {
        Err(errs)
    }
}

fn signature_of(f: &ast::FnItem, sigs: &HashMap<String, (Vec<Ty>, Ty)>) -> Signature {
    let (params, ret) = sigs
        .get(&f.name)
        .cloned()
        .unwrap_or_else(|| (Vec::new(), Ty::Unit));
    Signature {
        name: f.name.clone(),
        params: f
            .params
            .iter()
            .zip(&params)
            .map(|((n, _), t)| (n.clone(), t.tir()))
            .collect(),
        ret: if ret == Ty::Unit {
            None
        } else {
            Some(ret.tir())
        },
    }
}

fn resolve_ty(t: &ast::Ty) -> R<Ty> {
    match t {
        ast::Ty::Unit(_) => Ok(Ty::Unit),
        ast::Ty::Name(name, line) => match name.as_str() {
            "trit" => Ok(Ty::Trit),
            "bool" => Ok(Ty::Bool),
            "t9" => Ok(Ty::T9),
            "t27" => Ok(Ty::T27),
            "taddr" => Ok(Ty::TAddr),
            // Ch. 1 §8 claims these so no user identifier can take them.
            "t3" | "t81" | "f27" => err(
                *line,
                format!("`{name}` is a reserved type name (Ch. 1 §8)"),
            ),
            other => err(*line, format!("`{other}` is not a type in scope")),
        },
        ast::Ty::Array(elem, count, line) => {
            let elem = resolve_ty(elem)?;
            let n = const_int(count)?;
            if n < 0 {
                // Ch. 2 §3: the type-level face of the signed-taddr decision.
                return err(*line, format!("array length {n} is negative"));
            }
            Ok(Ty::Array(Box::new(elem), n))
        }
    }
}

/// Evaluate a constant expression. Ch. 0 §3.2: exactly, in balanced ternary.
fn const_int(e: &ast::Expr) -> R<i128> {
    match e {
        ast::Expr::Int(v, line) => v
            .to_i128()
            .ok_or(())
            .or_else(|()| err(*line, format!("{v} is too large"))),
        other => err(
            other.line(),
            "this milestone evaluates only integer literals in constant position",
        ),
    }
}

fn const_item(c: &ast::ConstItem, module: &mut Module) -> R<Global> {
    let ty = resolve_ty(&c.ty)?;
    match (&ty, &c.value) {
        (Ty::Array(elem, n), ast::Expr::Array(items, line)) => {
            if items.len() as i128 != *n {
                return err(
                    *line,
                    format!("expected {n} elements, found {}", items.len()),
                );
            }
            let mut trytes = Vec::new();
            for item in items {
                let v = const_int(item)?;
                let width = elem.width().unwrap_or(27);
                if !Bt::from_i128(v).fits_width(width) {
                    return err(item.line(), format!("{v} does not fit in {elem}"));
                }
                // Little-trytean, like every other multi-tryte value.
                let value = Bt::from_i128(v);
                for i in 0..elem.size() {
                    trytes.push(value.shr(i as u32 * 9).wrap_to(9));
                }
            }
            let name = format!("const.{}", c.name);
            module.globals.push(ir::Global {
                name: name.clone(),
                trytes: trytes.len() as u32,
                init: Some(trytes),
            });
            Ok(Global::Array(name, ty))
        }
        (t, _) if t.is_scalar() => {
            let v = const_int(&c.value)?;
            let width = t.width().unwrap_or(27);
            if !Bt::from_i128(v).fits_width(width) {
                return err(c.value.line(), format!("{v} does not fit in {t}"));
            }
            Ok(Global::Const(Bt::from_i128(v), ty))
        }
        _ => err(c.line, "this constant's form is not supported yet"),
    }
}

// --------------------------------------------------------------- lowering

struct Fn<'a> {
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
    globals: &'a HashMap<String, Global>,
    ret: Ty,

    blocks: Vec<Block>,
    label: String,
    insts: Vec<Inst>,
    /// Slot allocations, which all live in the entry block so that they
    /// dominate every use.
    slots: Vec<Inst>,
    scopes: Vec<HashMap<String, Local>>,
    loops: Vec<LoopCtx>,
    counter: u32,
    /// Set once the current block has been terminated.
    done: bool,
}

#[derive(Clone)]
struct LoopCtx {
    /// Where `break` goes.
    exit: String,
    /// Where `continue` goes.
    head: String,
    /// The slot a `break` with a value writes to.
    result: Option<(String, Ty)>,
}

fn function(
    f: &ast::FnItem,
    sig: Signature,
    body: &ast::Block,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    globals: &HashMap<String, Global>,
) -> R<Function> {
    let (param_tys, ret) = sigs.get(&f.name).cloned().unwrap();
    let mut fx = Fn {
        sigs,
        globals,
        ret: ret.clone(),
        blocks: Vec::new(),
        label: "entry".to_string(),
        insts: Vec::new(),
        slots: Vec::new(),
        scopes: vec![HashMap::new()],
        loops: Vec::new(),
        counter: 0,
        done: false,
    };

    // Parameters arrive as SSA values and are spilled into slots at once, so
    // that they read and write like any other local.
    for ((name, _), ty) in f.params.iter().zip(&param_tys) {
        let local = fx.declare(name, ty.clone(), true);
        fx.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: ty.tir(),
                v: Operand::Value(name.clone()),
                p: Operand::Value(local.slot),
            },
        });
    }

    let (value, ty) = fx.block(body, Some(&ret))?;
    if !fx.done {
        if ret == Ty::Unit {
            fx.finish(Terminator::Ret(None));
        } else {
            fx.check(&ty, &ret, body.line, "function body")?;
            fx.finish(Terminator::Ret(Some(value)));
        }
    }

    // The slots are prepended to the entry block, where they dominate
    // everything.
    let mut blocks = fx.blocks;
    let mut entry = blocks.remove(0);
    let mut insts = std::mem::take(&mut fx.slots);
    insts.append(&mut entry.insts);
    entry.insts = insts;
    blocks.insert(0, entry);

    Ok(Function { sig, blocks })
}

impl Fn<'_> {
    // ------------------------------------------------------- block building

    fn fresh(&mut self, what: &str) -> String {
        self.counter += 1;
        format!("{what}.{}", self.counter)
    }

    fn push(&mut self, inst: Inst) {
        if !self.done {
            self.insts.push(inst);
        }
    }

    fn emit(&mut self, what: &str, ty: Type, kind: InstKind) -> Operand {
        let name = self.fresh(what);
        self.push(Inst {
            results: vec![name.clone()],
            kind,
        });
        let _ = ty;
        Operand::Value(name)
    }

    fn finish(&mut self, term: Terminator) {
        if self.done {
            return;
        }
        self.blocks.push(Block {
            label: std::mem::take(&mut self.label),
            params: Vec::new(),
            insts: std::mem::take(&mut self.insts),
            term,
        });
        self.done = true;
    }

    fn start(&mut self, label: String) {
        self.label = label;
        self.insts = Vec::new();
        self.done = false;
    }

    fn jump(&mut self, label: &str) {
        self.finish(Terminator::Br(Target {
            label: label.to_string(),
            args: Vec::new(),
        }));
    }

    /// A three-way branch on a trit-valued operand.
    fn br3(&mut self, t: Operand, neg: &str, zero: &str, pos: &str) {
        let target = |l: &str| Target {
            label: l.to_string(),
            args: Vec::new(),
        };
        self.finish(Terminator::Br3 {
            t,
            neg: target(neg),
            zero: target(zero),
            pos: target(pos),
        });
    }

    // ------------------------------------------------------------- scopes

    fn declare(&mut self, name: &str, ty: Ty, _mutable: bool) -> Local {
        let slot = self.fresh(&format!("{name}.slot"));
        let trytes = ty.size().max(1) as u32;
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot { trytes },
        });
        let local = Local {
            slot,
            ty,
            mutable: _mutable,
        };
        self.scopes
            .last_mut()
            .expect("a scope")
            .insert(name.to_string(), local.clone());
        local
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
    }

    // -------------------------------------------------------------- checks

    fn check(&self, got: &Ty, want: &Ty, line: Line, what: &str) -> R<()> {
        if got == want || *got == Ty::Never {
            return Ok(());
        }
        err(
            line,
            format!("{what} has type {got}, expected {want} (there are no implicit conversions)"),
        )
    }

    // --------------------------------------------------------------- items

    fn block(&mut self, b: &ast::Block, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        self.scopes.push(HashMap::new());
        for stmt in &b.stmts {
            self.stmt(stmt)?;
        }
        let result = match &b.tail {
            Some(e) => self.expr(e, expected)?,
            None => (unit(), Ty::Unit),
        };
        self.scopes.pop();
        Ok(result)
    }

    fn stmt(&mut self, s: &ast::Stmt) -> R<()> {
        match s {
            ast::Stmt::Let {
                mutable,
                name,
                ty,
                value,
                line,
            } => {
                let declared = ty.as_ref().map(resolve_ty).transpose()?;
                let (v, vt) = self.expr(value, declared.as_ref())?;
                let ty = match declared {
                    Some(d) => {
                        self.check(&vt, &d, *line, "initializer")?;
                        d
                    }
                    None if vt == Ty::Never || vt == Ty::Unit => {
                        return err(*line, format!("cannot bind a value of type {vt}"));
                    }
                    None => vt,
                };
                let local = self.declare(name, ty.clone(), *mutable);
                if ty.is_scalar() {
                    self.push(Inst {
                        results: Vec::new(),
                        kind: InstKind::Store {
                            ty: ty.tir(),
                            v,
                            p: Operand::Value(local.slot),
                        },
                    });
                } else {
                    return err(
                        *line,
                        "only scalar bindings are supported in this milestone",
                    );
                }
                Ok(())
            }
            ast::Stmt::Expr(e) => {
                self.expr(e, None)?;
                Ok(())
            }
        }
    }

    // --------------------------------------------------------- expressions

    fn expr(&mut self, e: &ast::Expr, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        use ast::Expr as E;
        match e {
            // An unconstrained integer literal is `t27` (Ch. 1 §3), and one
            // that does not fit its type is an error, never a wrap.
            E::Int(v, line) => {
                let ty = match expected {
                    Some(t) if t.is_arithmetic() => t.clone(),
                    _ => Ty::T27,
                };
                let width = ty.width().unwrap_or(27);
                if !v.fits_width(width) {
                    return err(*line, format!("{v} does not fit in {ty}"));
                }
                Ok((Operand::Const(ty.tir(), v.clone()), ty))
            }

            E::Trit(t, _) => Ok((Operand::Const(Type::Int(1), Bt::from(*t)), Ty::Trit)),

            // false = 0, true = 1 in a full tryte (Ch. 1 §2).
            E::Bool(b, _) => Ok((
                Operand::Const(Type::Int(1), Bt::from_i128(i128::from(*b))),
                Ty::Bool,
            )),

            E::Unit(_) => Ok((unit(), Ty::Unit)),

            E::Path(name, line) => {
                if let Some(local) = self.lookup(name) {
                    if !local.ty.is_scalar() {
                        // An array's value is its address.
                        return Ok((Operand::Value(local.slot), local.ty));
                    }
                    let ty = local.ty.clone();
                    let v = self.emit(
                        "v",
                        ty.tir(),
                        InstKind::Load {
                            ty: ty.tir(),
                            p: Operand::Value(local.slot),
                        },
                    );
                    return Ok((v, ty));
                }
                match self.globals.get(name) {
                    Some(Global::Const(v, ty)) => {
                        Ok((Operand::Const(ty.tir(), v.clone()), ty.clone()))
                    }
                    Some(Global::Array(sym, ty)) => Ok((Operand::Global(sym.clone()), ty.clone())),
                    None => err(*line, format!("`{name}` is not in scope")),
                }
            }

            E::Unary(op, inner, line) => self.unary(op, inner, expected, *line),
            E::Binary(op, a, b, line) => self.binary(op, a, b, expected, *line),
            E::Assign(op, target, value, line) => self.assign(op, target, value, *line),
            E::Cast(inner, ty, line) => self.cast(inner, ty, *line),
            E::Index(base, index, line) => self.index(base, index, *line),
            E::Call(name, args, line) => self.call(name, args, *line),
            E::Method(recv, name, args, line) => self.method(recv, name, args, *line),

            E::Block(b) => self.block(b, expected),
            E::If(cond, then, els, line) => {
                self.if_expr(cond, then, els.as_deref(), expected, *line)
            }
            E::Match(scrutinee, arms, line) => self.match_expr(scrutinee, arms, expected, *line),
            E::While(cond, body, line) => self.while_expr(cond, body, *line),
            E::Loop(body, line) => self.loop_expr(body, expected, *line),

            E::Break(value, line) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*line, "`break` outside a loop");
                };
                match (value, &ctx.result) {
                    (Some(v), Some((slot, ty))) => {
                        let (val, vt) = self.expr(v, Some(ty))?;
                        self.check(&vt, ty, *line, "`break` value")?;
                        let (slot, ty) = (slot.clone(), ty.clone());
                        self.push(Inst {
                            results: Vec::new(),
                            kind: InstKind::Store {
                                ty: ty.tir(),
                                v: val,
                                p: Operand::Value(slot),
                            },
                        });
                    }
                    (Some(_), None) => {
                        return err(*line, "this loop's `break` cannot carry a value");
                    }
                    (None, _) => {}
                }
                self.jump(&ctx.exit);
                Ok((unit(), Ty::Never))
            }

            E::Continue(line) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*line, "`continue` outside a loop");
                };
                self.jump(&ctx.head);
                Ok((unit(), Ty::Never))
            }

            E::Return(value, line) => {
                let ret = self.ret.clone();
                match value {
                    Some(v) => {
                        let (val, vt) = self.expr(v, Some(&ret))?;
                        self.check(&vt, &ret, *line, "returned value")?;
                        self.finish(Terminator::Ret(Some(val)));
                    }
                    None => {
                        self.check(&Ty::Unit, &ret, *line, "`return` with no value")?;
                        self.finish(Terminator::Ret(None));
                    }
                }
                Ok((unit(), Ty::Never))
            }

            E::Array(_, line) | E::Repeat(_, _, line) => err(
                *line,
                "array expressions are supported only as constant initializers in this milestone",
            ),
        }
    }

    fn unary(
        &mut self,
        op: &str,
        inner: &ast::Expr,
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(inner, expected)?;
        match op {
            // Negation is total at every width (Ch. 1 §1, P1).
            "-" if ty.is_arithmetic() || ty == Ty::Trit => {
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }
            "-" => err(line, format!("`-` does not apply to {ty}")),
            "!" if ty == Ty::Bool => {
                // `false` is 0 and `true` is 1, so negation is `1 − b`.
                let one = Operand::Const(Type::Int(1), Bt::from_i128(1));
                let r = self.emit(
                    "n",
                    Type::Int(1),
                    InstKind::Flavored {
                        op: FlavoredOp::Sub,
                        flavor: Flavor::Wrap,
                        ty: Type::Int(1),
                        a: one,
                        b: v,
                    },
                );
                Ok((r, Ty::Bool))
            }
            "!" => err(line, format!("`!` applies to bool, not {ty}")),
            _ => err(line, format!("unknown unary operator `{op}`")),
        }
    }

    fn binary(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        // Short-circuit operators are control flow, not arithmetic.
        if op == "&&" || op == "||" {
            return self.short_circuit(op, a, b, line);
        }

        let arith_hint = expected.filter(|t| t.is_arithmetic());
        let (va, ta) = self.expr(a, arith_hint)?;
        let (vb, tb) = self.expr(b, Some(&ta))?;
        // Mixed-width arithmetic is a compile-time error (Ch. 1, P2).
        self.check(&tb, &ta, line, "right operand")?;

        let tir = ta.tir();
        match op {
            "+" | "-" | "*" | "<<" => {
                if !ta.is_arithmetic() {
                    return err(line, format!("`{op}` does not apply to {ta}"));
                }
                let fop = match op {
                    "+" => FlavoredOp::Add,
                    "-" => FlavoredOp::Sub,
                    "*" => FlavoredOp::Mul,
                    _ => FlavoredOp::Shl,
                };
                // Ch. 1, P4: the default profile traps on overflow. A release
                // profile would select `.wrap` here, and the `wrapping_*`
                // methods are always available.
                let r = self.emit(
                    "a",
                    tir,
                    InstKind::Flavored {
                        op: fop,
                        flavor: Flavor::Trap,
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, ta))
            }

            "/" | "%" | ">>" => {
                if !ta.is_arithmetic() {
                    return err(line, format!("`{op}` does not apply to {ta}"));
                }
                let pop = match op {
                    "/" => PlainOp::Div,
                    "%" => PlainOp::Rem,
                    _ => PlainOp::Shr,
                };
                let r = self.emit(
                    "a",
                    tir,
                    InstKind::Plain {
                        op: pop,
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, ta))
            }

            // The primitive comparison (Ch. 1 §5).
            "<=>" => {
                let r = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, Ty::Trit))
            }

            // The two-way predicates are projections of it: one comparison,
            // then a three-way select of 0 or 1.
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                let c = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                let (n, z, p) = match op {
                    "==" => (0, 1, 0),
                    "!=" => (1, 0, 1),
                    "<" => (1, 0, 0),
                    "<=" => (1, 1, 0),
                    ">" => (0, 0, 1),
                    _ => (0, 1, 1),
                };
                let k = |v: i128| Operand::Const(Type::Int(1), Bt::from_i128(v));
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: c,
                        ty: Type::Int(1),
                        neg: k(n),
                        zero: k(z),
                        pos: k(p),
                    },
                );
                Ok((r, Ty::Bool))
            }

            _ => err(line, format!("unknown operator `{op}`")),
        }
    }

    /// `&&` and `||` short-circuit, so they are branches (Ch. 0 §2.4).
    fn short_circuit(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (va, ta) = self.expr(a, Some(&Ty::Bool))?;
        self.check(&ta, &Ty::Bool, line, "left operand")?;

        let slot = self.temp_slot(&Ty::Bool);
        let rhs = self.fresh("sc.rhs");
        let join = self.fresh("sc.join");
        let short = self.fresh("sc.short");

        // Store the short-circuit answer, then test.
        self.store_slot(&slot, &Ty::Bool, va.clone());
        if op == "&&" {
            self.br3(va, &short, &short, &rhs);
        } else {
            self.br3(va, &rhs, &rhs, &short);
        }

        self.start(short);
        self.jump(&join);

        self.start(rhs);
        let (vb, tb) = self.expr(b, Some(&Ty::Bool))?;
        self.check(&tb, &Ty::Bool, line, "right operand")?;
        self.store_slot(&slot, &Ty::Bool, vb);
        self.jump(&join);

        self.start(join);
        let v = self.load_slot(&slot, &Ty::Bool);
        Ok((v, Ty::Bool))
    }

    fn assign(
        &mut self,
        op: &str,
        target: &ast::Expr,
        value: &ast::Expr,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let ast::Expr::Path(name, _) = target else {
            return err(line, "only a local may be assigned in this milestone");
        };
        let Some(local) = self.lookup(name) else {
            return err(line, format!("`{name}` is not in scope"));
        };
        if !local.mutable {
            return err(
                line,
                format!("`{name}` is not mutable; declare it `let mut {name}`"),
            );
        }

        let ty = local.ty.clone();
        let v = if op == "=" {
            let (v, vt) = self.expr(value, Some(&ty))?;
            self.check(&vt, &ty, line, "assigned value")?;
            v
        } else {
            // `a op= b` is `a = a op b`, with `a` evaluated once (§2.2).
            let binop = &op[..op.len() - 1];
            let current = self.load_slot(&local.slot, &ty);
            let (rhs, rt) = self.expr(value, Some(&ty))?;
            self.check(&rt, &ty, line, "assigned value")?;
            self.apply_binary(binop, current, rhs, &ty, line)?
        };
        self.store_slot(&local.slot, &ty, v);
        Ok((unit(), Ty::Unit))
    }

    /// The arithmetic half of `binary`, for compound assignment.
    fn apply_binary(
        &mut self,
        op: &str,
        a: Operand,
        b: Operand,
        ty: &Ty,
        line: Line,
    ) -> R<Operand> {
        if !ty.is_arithmetic() {
            return err(line, format!("`{op}=` does not apply to {ty}"));
        }
        let tir = ty.tir();
        Ok(match op {
            "+" | "-" | "*" | "<<" => {
                let fop = match op {
                    "+" => FlavoredOp::Add,
                    "-" => FlavoredOp::Sub,
                    "*" => FlavoredOp::Mul,
                    _ => FlavoredOp::Shl,
                };
                self.emit(
                    "a",
                    tir,
                    InstKind::Flavored {
                        op: fop,
                        flavor: Flavor::Trap,
                        ty: tir,
                        a,
                        b,
                    },
                )
            }
            "/" | "%" | ">>" => {
                let pop = match op {
                    "/" => PlainOp::Div,
                    "%" => PlainOp::Rem,
                    _ => PlainOp::Shr,
                };
                self.emit(
                    "a",
                    tir,
                    InstKind::Plain {
                        op: pop,
                        ty: tir,
                        a,
                        b,
                    },
                )
            }
            _ => return err(line, format!("unknown operator `{op}=`")),
        })
    }

    /// `x as T` (Ch. 1 §6).
    fn cast(&mut self, inner: &ast::Expr, to: &ast::Ty, line: Line) -> R<(Operand, Ty)> {
        let to = resolve_ty(to)?;
        let (v, from) = self.expr(inner, None)?;

        // `trit` ↔ `bool` has no `as` path by design: both mappings are
        // plausible, so the language refuses to pick (Ch. 1 §6).
        if (from == Ty::Trit && to == Ty::Bool) || (from == Ty::Bool && to == Ty::Trit) {
            return err(
                line,
                "there is no `as` between `trit` and `bool`: use `is_pos`/`is_zero`/`is_neg` \
                 or `to_trit` (Ch. 1 §6)",
            );
        }
        let (Some(fw), Some(tw)) = (from.width(), to.width()) else {
            return err(line, format!("cannot cast {from} to {to}"));
        };
        let value = match fw.cmp(&tw) {
            std::cmp::Ordering::Less => self.emit(
                "w",
                to.tir(),
                InstKind::Widen {
                    from: from.tir(),
                    a: v,
                    to: to.tir(),
                },
            ),
            std::cmp::Ordering::Greater => self.emit(
                "t",
                to.tir(),
                InstKind::Trunc {
                    from: from.tir(),
                    a: v,
                    to: to.tir(),
                },
            ),
            std::cmp::Ordering::Equal => v,
        };
        Ok((value, to))
    }

    /// `base[index]` — with the bounds check Ch. 2 §3 requires.
    fn index(&mut self, base: &ast::Expr, index: &ast::Expr, line: Line) -> R<(Operand, Ty)> {
        let (addr, bt) = self.expr(base, None)?;
        let Ty::Array(elem, n) = bt else {
            return err(line, format!("{bt} cannot be indexed"));
        };
        let (idx, it) = self.expr(index, Some(&Ty::TAddr))?;
        self.check(&it, &Ty::TAddr, line, "index")?;

        let word = Type::Int(27);
        let k = |v: i128| Operand::Const(word, Bt::from_i128(v));

        // Two checks, not the fused one Ch. 2 §3 suggests — see the note in
        // docs/spec-gaps.md: `tmin(sign(i), i <=> N)` is −1 for an in-bounds
        // index as well as an out-of-bounds one, so it cannot distinguish
        // them.
        let low = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: word,
                a: idx.clone(),
                b: k(0),
            },
        );
        let (ok_low, fault) = (self.fresh("idx.lo"), self.fresh("idx.fault"));
        self.br3(low, &fault, &ok_low, &ok_low);

        self.start(fault.clone());
        self.finish(Terminator::Trap(FaultCode::Trap));

        self.start(ok_low);
        let high = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: word,
                a: idx.clone(),
                b: k(n),
            },
        );
        let ok = self.fresh("idx.ok");
        self.br3(high, &ok, &fault, &fault);

        self.start(ok);
        let scale = self.emit(
            "a",
            word,
            InstKind::Flavored {
                op: FlavoredOp::Mul,
                flavor: Flavor::Trap,
                ty: word,
                a: idx,
                b: k(elem.size()),
            },
        );
        let p = self.emit("p", Type::Ptr, InstKind::Offset { p: addr, d: scale });
        let v = self.emit("v", elem.tir(), InstKind::Load { ty: elem.tir(), p });
        Ok((v, *elem))
    }

    fn call(&mut self, name: &str, args: &[ast::Expr], line: Line) -> R<(Operand, Ty)> {
        // `sign(x)` is a function, not a method (Ch. 1 §6).
        if name == "sign" {
            if args.len() != 1 {
                return err(line, "`sign` takes one argument");
            }
            let (v, ty) = self.expr(&args[0], None)?;
            if !ty.is_arithmetic() && ty != Ty::Trit {
                return err(line, format!("`sign` does not apply to {ty}"));
            }
            let zero = Operand::Const(ty.tir(), Bt::ZERO);
            let r = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: ty.tir(),
                    a: v,
                    b: zero,
                },
            );
            return Ok((r, Ty::Trit));
        }

        let Some((params, ret)) = self.sigs.get(name).cloned() else {
            return err(line, format!("`{name}` is not a function in scope"));
        };
        if params.len() != args.len() {
            return err(
                line,
                format!(
                    "`{name}` takes {} argument(s), {} given",
                    params.len(),
                    args.len()
                ),
            );
        }
        let mut values = Vec::new();
        for (arg, want) in args.iter().zip(&params) {
            let (v, got) = self.expr(arg, Some(want))?;
            self.check(&got, want, arg.line(), "argument")?;
            values.push(v);
        }
        let kind = InstKind::Call {
            callee: name.to_string(),
            args: values,
            ret: if ret == Ty::Unit {
                None
            } else {
                Some(ret.tir())
            },
        };
        if ret == Ty::Unit {
            self.push(Inst {
                results: Vec::new(),
                kind,
            });
            Ok((unit(), Ty::Unit))
        } else {
            let v = self.emit("r", ret.tir(), kind);
            Ok((v, ret))
        }
    }

    /// The method forms Ch. 1 fixes: the trit-wise operations and the
    /// `trit` ↔ `bool` projections (Ch. 1 §4, §6).
    fn method(
        &mut self,
        recv: &ast::Expr,
        name: &str,
        args: &[ast::Expr],
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(recv, None)?;
        let one_arg = |this: &mut Self, want: &Ty| -> R<Operand> {
            if args.len() != 1 {
                return err(line, format!("`{name}` takes one argument"));
            }
            let (a, at) = this.expr(&args[0], Some(want))?;
            this.check(&at, want, line, "argument")?;
            Ok(a)
        };

        match name {
            "tmin" | "tmax" | "tmul" => {
                if !ty.is_arithmetic() && ty != Ty::Trit {
                    return err(line, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name {
                    "tmin" => PlainOp::TMin,
                    "tmax" => PlainOp::TMax,
                    _ => PlainOp::TMul,
                };
                let r = self.emit(
                    "a",
                    ty.tir(),
                    InstKind::Plain {
                        op,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                );
                Ok((r, ty))
            }

            // `tneg` is identical to unary `-`; provided for symmetry when
            // writing trit-manipulation code (Ch. 1 §4).
            "tneg" => {
                if !args.is_empty() {
                    return err(line, "`tneg` takes no arguments");
                }
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }

            "is_pos" | "is_zero" | "is_neg" => {
                if ty != Ty::Trit {
                    return err(line, format!("`{name}` applies to trit, not {ty}"));
                }
                if !args.is_empty() {
                    return err(line, format!("`{name}` takes no arguments"));
                }
                let k = |v: i128| Operand::Const(Type::Int(1), Bt::from_i128(v));
                let (n, z, p) = match name {
                    "is_neg" => (1, 0, 0),
                    "is_zero" => (0, 1, 0),
                    _ => (0, 0, 1),
                };
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: v,
                        ty: Type::Int(1),
                        neg: k(n),
                        zero: k(z),
                        pos: k(p),
                    },
                );
                Ok((r, Ty::Bool))
            }

            // false → 0t, true → 1t. The representations already agree, so
            // this is a retype and not an operation.
            "to_trit" => {
                if ty != Ty::Bool {
                    return err(line, format!("`to_trit` applies to bool, not {ty}"));
                }
                Ok((v, Ty::Trit))
            }

            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => {
                if !ty.is_arithmetic() {
                    return err(line, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name {
                    "wrapping_add" => FlavoredOp::Add,
                    "wrapping_sub" => FlavoredOp::Sub,
                    _ => FlavoredOp::Mul,
                };
                let r = self.emit(
                    "a",
                    ty.tir(),
                    InstKind::Flavored {
                        op,
                        flavor: Flavor::Wrap,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                );
                Ok((r, ty))
            }

            "checked_add" | "checked_sub" | "checked_mul" | "overflowing_add"
            | "overflowing_sub" | "overflowing_mul" => err(
                line,
                format!(
                    "`{name}` returns an Option or a tuple, neither of which is in this milestone"
                ),
            ),

            other => err(line, format!("`{other}` is not a method in this milestone")),
        }
    }

    // ------------------------------------------------------ control flow

    fn temp_slot(&mut self, ty: &Ty) -> String {
        let slot = self.fresh("tmp.slot");
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot {
                trytes: ty.size().max(1) as u32,
            },
        });
        slot
    }

    fn store_slot(&mut self, slot: &str, ty: &Ty, v: Operand) {
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: ty.tir(),
                v,
                p: Operand::Value(slot.to_string()),
            },
        });
    }

    fn load_slot(&mut self, slot: &str, ty: &Ty) -> Operand {
        self.emit(
            "v",
            ty.tir(),
            InstKind::Load {
                ty: ty.tir(),
                p: Operand::Value(slot.to_string()),
            },
        )
    }

    fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then: &ast::Block,
        els: Option<&ast::Expr>,
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        // The condition is a `bool`, not a `trit`, and not "anything
        // nonzero" (Ch. 1 §2).
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, line, "condition")?;

        let (then_l, else_l, join_l) = (self.fresh("then"), self.fresh("else"), self.fresh("join"));
        self.br3(c, &else_l, &else_l, &then_l);

        // The two arms' values meet in a slot, so nothing crosses a block
        // edge in a register.
        let mut result: Option<(String, Ty)> = None;

        self.start(then_l);
        let (tv, tt) = self.block(then, expected)?;
        if tt != Ty::Never && tt != Ty::Unit {
            let slot = self.temp_slot(&tt);
            self.store_slot(&slot, &tt, tv);
            result = Some((slot, tt.clone()));
        }
        self.jump(&join_l);

        self.start(else_l);
        let et = match els {
            None => Ty::Unit,
            Some(e) => {
                let (ev, et) = self.expr(e, expected)?;
                if let Some((slot, ty)) = &result
                    && et != Ty::Never
                {
                    let (slot, ty) = (slot.clone(), ty.clone());
                    self.check(&et, &ty, line, "`else` branch")?;
                    self.store_slot(&slot, &ty, ev);
                }
                et
            }
        };
        self.jump(&join_l);

        self.start(join_l);
        match result {
            // An `if` used for its value must have an `else` (§5.3).
            Some((slot, ty)) if els.is_some() => {
                let v = self.load_slot(&slot, &ty);
                Ok((v, ty))
            }
            Some((_, ty)) if ty != Ty::Unit && expected.is_some_and(|e| *e != Ty::Unit) => {
                err(line, "an `if` used for its value needs an `else` branch")
            }
            _ => {
                if tt == Ty::Never && et == Ty::Never {
                    // Both arms diverge; so does the `if`.
                    return Ok((unit(), Ty::Never));
                }
                Ok((unit(), Ty::Unit))
            }
        }
    }

    fn while_expr(&mut self, cond: &ast::Expr, body: &ast::Block, line: Line) -> R<(Operand, Ty)> {
        let (head, body_l, exit) = (self.fresh("while"), self.fresh("body"), self.fresh("done"));
        self.jump(&head);

        self.start(head.clone());
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, line, "condition")?;
        self.br3(c, &exit, &exit, &body_l);

        self.start(body_l);
        self.loops.push(LoopCtx {
            exit: exit.clone(),
            head: head.clone(),
            result: None,
        });
        self.block(body, None)?;
        self.loops.pop();
        self.jump(&head);

        self.start(exit);
        Ok((unit(), Ty::Unit))
    }

    fn loop_expr(
        &mut self,
        body: &ast::Block,
        expected: Option<&Ty>,
        _line: Line,
    ) -> R<(Operand, Ty)> {
        let (head, exit) = (self.fresh("loop"), self.fresh("done"));
        // A `loop` yields a value only if something expects one; `break expr`
        // then writes to this slot.
        let result = expected
            .filter(|t| t.is_scalar())
            .map(|t| (self.temp_slot(t), t.clone()));

        self.jump(&head);
        self.start(head.clone());
        self.loops.push(LoopCtx {
            exit: exit.clone(),
            head: head.clone(),
            result: result.clone(),
        });
        self.block(body, None)?;
        self.loops.pop();
        self.jump(&head);

        self.start(exit);
        match result {
            Some((slot, ty)) => {
                let v = self.load_slot(&slot, &ty);
                Ok((v, ty))
            }
            None => Ok((unit(), Ty::Unit)),
        }
    }

    /// `match` (§5.4). Over a `trit` with the three trit patterns this is one
    /// `br3` and nothing else — which is the whole point.
    fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::Arm],
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(scrutinee, None)?;
        if !ty.is_scalar() {
            return err(line, format!("cannot match on {ty}"));
        }
        check_exhaustive(&ty, arms, line)?;

        let join = self.fresh("match.join");
        let mut result: Option<(String, Ty)> = None;

        // A trit scrutinee whose arms are the three trit literals is exactly
        // a three-way branch (Ch. 1 §5).
        if ty == Ty::Trit
            && let Some(order) = trit_dispatch(arms)
        {
            let labels: Vec<String> = (0..arms.len()).map(|_| self.fresh("arm")).collect();
            let pick = |i: Option<usize>| match i {
                Some(i) => labels[i].clone(),
                None => join.clone(),
            };
            self.br3(v, &pick(order[0]), &pick(order[1]), &pick(order[2]));
            for (arm, label) in arms.iter().zip(&labels) {
                self.start(label.clone());
                self.arm_body(arm, expected, &mut result, &join, line)?;
            }
            self.start(join);
            return Ok(self.match_result(result));
        }

        // Otherwise: test the arms in order.
        let mut fell_through = true;
        for arm in arms {
            let next = self.fresh("arm.next");
            let body = self.fresh("arm");
            let unconditional = self.arm_test(arm, &v, &ty, &body, &next, line)?;
            self.start(body);
            self.arm_body(arm, expected, &mut result, &join, line)?;
            if unconditional {
                // A wildcard or binding matches everything, so no later arm
                // and no fallthrough block is reachable — and an unreachable
                // block is one the verifier rejects.
                fell_through = false;
                break;
            }
            self.start(next);
        }
        if fell_through {
            // Exhaustiveness was checked, so control cannot arrive here in a
            // well-typed program — but TIR wants a terminator.
            self.finish(Terminator::Trap(FaultCode::Trap));
        }

        self.start(join);
        Ok(self.match_result(result))
    }

    fn match_result(&mut self, result: Option<(String, Ty)>) -> (Operand, Ty) {
        match result {
            Some((slot, ty)) => {
                let v = self.load_slot(&slot, &ty);
                (v, ty)
            }
            None => (unit(), Ty::Unit),
        }
    }

    fn arm_body(
        &mut self,
        arm: &ast::Arm,
        expected: Option<&Ty>,
        result: &mut Option<(String, Ty)>,
        join: &str,
        line: Line,
    ) -> R<()> {
        // A binding pattern names the scrutinee; this milestone's patterns
        // bind nothing else.
        let (v, ty) = self.expr(&arm.body, expected)?;
        if ty != Ty::Never && ty != Ty::Unit {
            if result.is_none() {
                *result = Some((self.temp_slot(&ty), ty.clone()));
            }
            let (slot, want) = result.clone().expect("just set");
            self.check(&ty, &want, line, "match arm")?;
            self.store_slot(&slot, &want, v);
        }
        self.jump(join);
        Ok(())
    }

    /// Emit the test for one arm, branching to `body` or `next`. Returns
    /// true if the arm matches unconditionally.
    fn arm_test(
        &mut self,
        arm: &ast::Arm,
        v: &Operand,
        ty: &Ty,
        body: &str,
        next: &str,
        line: Line,
    ) -> R<bool> {
        if arm.guard.is_some() {
            return err(line, "match guards are not lowered yet");
        }
        // A wildcard or binding matches everything.
        if arm
            .patterns
            .iter()
            .any(|p| matches!(p, ast::Pattern::Wild(_) | ast::Pattern::Bind(..)))
        {
            self.jump(body);
            return Ok(true);
        }
        let mut remaining = arm.patterns.len();
        for p in &arm.patterns {
            let value = pattern_value(p, ty, line)?;
            let k = Operand::Const(ty.tir(), value);
            let c = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: ty.tir(),
                    a: v.clone(),
                    b: k,
                },
            );
            remaining -= 1;
            let fail = if remaining == 0 {
                next.to_string()
            } else {
                self.fresh("alt")
            };
            self.br3(c, &fail, body, &fail);
            self.start(fail);
        }
        Ok(false)
    }
}

/// A unit value: never read, but TIR operands are always typed.
fn unit() -> Operand {
    Operand::Const(Type::Int(27), Bt::ZERO)
}

fn pattern_value(p: &ast::Pattern, ty: &Ty, line: Line) -> R<Bt> {
    match p {
        ast::Pattern::Int(v, l) => {
            if !ty.is_arithmetic() {
                return err(*l, format!("an integer pattern does not match {ty}"));
            }
            Ok(v.clone())
        }
        ast::Pattern::Trit(t, l) => {
            if *ty != Ty::Trit {
                return err(*l, format!("a trit pattern does not match {ty}"));
            }
            Ok(Bt::from(*t))
        }
        ast::Pattern::Bool(b, l) => {
            if *ty != Ty::Bool {
                return err(*l, format!("a bool pattern does not match {ty}"));
            }
            Ok(Bt::from_i128(i128::from(*b)))
        }
        _ => err(line, "unsupported pattern"),
    }
}

/// If the arms are exactly the three trit literals, in some order, return for
/// each of −1, 0, +1 the arm that handles it.
fn trit_dispatch(arms: &[ast::Arm]) -> Option<[Option<usize>; 3]> {
    let mut order = [None, None, None];
    for (i, arm) in arms.iter().enumerate() {
        if arm.guard.is_some() {
            return None;
        }
        for p in &arm.patterns {
            let ast::Pattern::Trit(t, _) = p else {
                return None;
            };
            let slot = (t.to_i8() + 1) as usize;
            if order[slot].is_some() {
                return None;
            }
            order[slot] = Some(i);
        }
    }
    order.iter().all(Option::is_some).then_some(order)
}

/// A `match` must be exhaustive (Ch. 2 §5). Over a `trit` that means three
/// arms or a wildcard; over `bool`, two; over an integer, a wildcard.
fn check_exhaustive(ty: &Ty, arms: &[ast::Arm], line: Line) -> R<()> {
    let mut seen: Vec<Bt> = Vec::new();
    for arm in arms {
        if arm.guard.is_some() {
            continue; // a guarded arm never counts (§5.4)
        }
        for p in &arm.patterns {
            match p {
                ast::Pattern::Wild(_) | ast::Pattern::Bind(..) => return Ok(()),
                _ => {
                    let v = pattern_value(p, ty, line)?;
                    if !seen.contains(&v) {
                        seen.push(v);
                    }
                }
            }
        }
    }
    let needed = match ty {
        Ty::Trit => 3,
        Ty::Bool => 2,
        _ => usize::MAX,
    };
    if seen.len() >= needed {
        Ok(())
    } else {
        err(
            line,
            format!(
                "this `match` is not exhaustive: {ty} needs {} more case(s) or a `_` arm",
                if needed == usize::MAX {
                    "some".to_string()
                } else {
                    (needed - seen.len()).to_string()
                }
            ),
        )
    }
}
