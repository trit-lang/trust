//! Macro expansion (Language Ch. 7).
//!
//! One pass, run before anything else reads the program (§5): after modules
//! are resolved, so a macro's body names what its own module names, and
//! before type aliases, so a body may expand to a use of one.
//!
//! Two things it has to get right, and they are the chapter's two decisions.
//!
//! **Hygiene** (§4). A name the body *binds* is a new name; a name the body
//! *uses* and does not bind is the macro's own. So every binding a body
//! introduces is renamed at each expansion to something no program can
//! write — the same device a closure's captures already use — and the
//! renaming reaches every use of that name inside the body, and nothing
//! outside it.
//!
//! **One rule** (§2). A parameter list, optionally ending in a repetition,
//! and one body. Expansion is substitution and nothing is matched, so the
//! only way an invocation can be wrong is by count.

use std::collections::HashMap;

use super::ast::{for_each_child_mut as walk_children, *};
use super::lex::{Span, SyntaxError};

/// Expand every invocation in a file, and drop the macros (§5).
pub fn expand(file: &mut File) -> Result<(), Vec<SyntaxError>> {
    let macros: HashMap<String, MacroItem> = file
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Macro(m) => Some((m.name.clone(), m.clone())),
            _ => None,
        })
        .collect();
    file.items.retain(|i| !matches!(i, Item::Macro(_)));
    // The walk happens even when nothing is defined: an invocation of a
    // macro that does not exist is the program's mistake, and skipping the
    // walk left it to be found below as a fault in this compiler.
    let mut x = Expander {
        macros,
        errors: Vec::new(),
        fresh: 0,
        depth: 0,
    };
    for item in &mut file.items {
        x.item(item);
    }
    if x.errors.is_empty() {
        Ok(())
    } else {
        Err(x.errors)
    }
}

/// How deep an expansion may nest before it is called a cycle.
///
/// §3.1 forbids recursion outright, so any depth beyond a handful is one
/// macro reaching another reaching another — legal, and bounded by how many
/// were written.
const DEPTH: u32 = 64;

struct Expander {
    macros: HashMap<String, MacroItem>,
    errors: Vec<SyntaxError>,
    /// Counts the bindings renamed, so two expansions of one macro do not
    /// collide with each other either.
    fresh: u32,
    depth: u32,
}

impl Expander {
    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(f) => {
                if let Some(b) = &mut f.body {
                    self.block(b);
                }
                for p in &mut f.requires {
                    self.expr(p);
                }
            }
            Item::Const(c) => self.expr(&mut c.value),
            Item::Trait(t) => {
                for m in &mut t.methods {
                    if let Some(b) = &mut m.body {
                        self.block(b);
                    }
                }
            }
            Item::Impl(i) => {
                for c in &mut i.consts {
                    self.expr(&mut c.value);
                }
                for m in &mut i.methods {
                    if let Some(b) = &mut m.body {
                        self.block(b);
                    }
                }
            }
            Item::Enum(_)
            | Item::Struct(_)
            | Item::Alias(_)
            | Item::Mod(_)
            | Item::Use(_)
            | Item::Macro(_) => {}
        }
    }

    fn block(&mut self, b: &mut Block) {
        let mut out: Vec<Stmt> = Vec::new();
        for mut s in b.stmts.drain(..) {
            match &mut s {
                Stmt::Let { value, .. } => self.expr(value),
                Stmt::Expr(e) => {
                    self.expr(e);
                    // A repetition in statement position expands to the
                    // statements it repeated, not to a block: `$( v.push(x); )*`
                    // is a sequence and a block would scope its bindings away.
                    if let Expr::MacroRepeat(stmts, _) = e {
                        out.append(stmts);
                        continue;
                    }
                }
            }
            out.push(s);
        }
        b.stmts = out;
        if let Some(t) = &mut b.tail {
            self.expr(t);
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match e {
            // A block is where a repetition is flattened, so every block
            // has to be reached as one rather than through the generic walk.
            Expr::Block(b) => self.block(b),
            Expr::If(c, then, other, _) => {
                self.expr(c);
                self.block(then);
                if let Some(o) = other {
                    self.expr(o);
                }
            }
            Expr::While(c, b, _) => {
                self.expr(c);
                self.block(b);
            }
            Expr::Loop(b, _) => self.block(b),
            _ => {
                // Children first, so an argument that is itself an
                // invocation is already expanded when it is substituted.
                walk_children(e, &mut |c| self.expr(c));
            }
        }
        let Expr::MacroCall(name, args, span) = e else {
            return;
        };
        let (name, args, span) = (name.clone(), std::mem::take(args), *span);
        match self.call(&name, args, span) {
            Ok(body) => *e = body,
            Err(err) => {
                self.errors.push(err);
                *e = Expr::Unit(span);
            }
        }
    }

    /// One invocation, expanded.
    fn call(&mut self, name: &str, args: Vec<Expr>, span: Span) -> Result<Expr, SyntaxError> {
        let Some(m) = self.macros.get(name).cloned() else {
            return Err(SyntaxError {
                span,
                message: format!("`{name}!` is not a macro in scope"),
            });
        };
        let fixed = m.params.len();
        let enough = match m.rest.is_some() {
            true => args.len() >= fixed,
            false => args.len() == fixed,
        };
        if !enough {
            return Err(SyntaxError {
                span,
                message: match m.rest.is_some() {
                    true => format!(
                        "`{name}!` takes at least {fixed} argument(s), {} given",
                        args.len()
                    ),
                    false => format!("`{name}!` takes {fixed} argument(s), {} given", args.len()),
                },
            });
        }
        self.depth += 1;
        if self.depth > DEPTH {
            self.depth -= 1;
            return Err(SyntaxError {
                span,
                message: format!(
                    "`{name}!` expands into itself, directly or through another macro \
                     (Ch. 7 §3.1)"
                ),
            });
        }

        let mut bound: HashMap<String, Expr> = HashMap::new();
        for (p, a) in m.params.iter().zip(&args) {
            bound.insert(p.clone(), a.clone());
        }
        let repeated: Vec<Expr> = args[fixed..].to_vec();

        // Hygiene: the body's own bindings are renamed before anything is
        // substituted into it, so a rename can never reach an argument.
        let mut body = m.body.clone();
        self.fresh += 1;
        let tag = self.fresh;
        let mut renames = HashMap::new();
        rename_bindings(&mut body, tag, &mut renames);

        let mut sub = Substitution {
            bound: &bound,
            rest: m.rest.as_deref(),
            repeated: &repeated,
            span,
            errors: Vec::new(),
        };
        sub.block(&mut body);
        let errors = std::mem::take(&mut sub.errors);
        if let Some(e) = errors.into_iter().next() {
            self.depth -= 1;
            return Err(e);
        }

        // The expansion is the body, and everything in it reports at the
        // invocation: that is the place a reader can act on (§5).
        let mut out = Expr::Block(body).spanning(span);
        self.expr(&mut out);
        self.depth -= 1;
        Ok(out)
    }
}

/// Give every binding a macro's body introduces a name no program can write.
///
/// A dot is what marks it: Ch. 0 §1.3's identifiers hold none, and the
/// compiler already forms names this way for closures and instantiations.
fn rename_bindings(b: &mut Block, tag: u32, renames: &mut HashMap<String, String>) {
    for s in &mut b.stmts {
        match s {
            Stmt::Let { name, value, .. } => {
                // The initializer is read before the name is bound (Ch. 0
                // §5.2), so a `let n = n;` in a body means the outer `n`.
                rename_uses_expr(value, renames);
                let to = format!("{name}.mac{tag}");
                renames.insert(name.clone(), to.clone());
                *name = to;
            }
            Stmt::Expr(e) => rename_expr(e, tag, renames),
        }
    }
    if let Some(t) = &mut b.tail {
        rename_expr(t, tag, renames);
    }
}

fn rename_expr(e: &mut Expr, tag: u32, renames: &mut HashMap<String, String>) {
    match e {
        Expr::Block(b) => {
            // A nested block's bindings end with it, exactly as they would
            // outside a macro.
            let mut inner = renames.clone();
            rename_bindings(b, tag, &mut inner);
        }
        Expr::Match(scrutinee, arms, _) => {
            rename_expr(scrutinee, tag, renames);
            for arm in arms {
                let mut inner = renames.clone();
                for p in &mut arm.patterns {
                    rename_pattern(p, tag, &mut inner);
                }
                if let Some(g) = &mut arm.guard {
                    rename_uses_expr(g, &inner);
                }
                rename_expr(&mut arm.body, tag, &mut inner);
            }
        }
        Expr::Closure(params, _, body, _) => {
            let mut inner = renames.clone();
            for (n, _) in params.iter_mut() {
                let to = format!("{n}.mac{tag}");
                inner.insert(n.clone(), to.clone());
                *n = to;
            }
            rename_expr(body, tag, &mut inner);
        }
        Expr::Path(n, _) => {
            if let Some(to) = renames.get(n) {
                *n = to.clone();
            }
        }
        other => walk_children(other, &mut |c| rename_expr(c, tag, renames)),
    }
}

/// Rename uses only — for a place where no new binding may appear.
fn rename_uses_expr(e: &mut Expr, renames: &HashMap<String, String>) {
    if let Expr::Path(n, _) = e
        && let Some(to) = renames.get(n)
    {
        *n = to.clone();
        return;
    }
    walk_children(e, &mut |c| rename_uses_expr(c, renames));
}

fn rename_pattern(p: &mut Pattern, tag: u32, renames: &mut HashMap<String, String>) {
    match p {
        Pattern::Bind(n, _) => {
            let to = format!("{n}.mac{tag}");
            renames.insert(n.clone(), to.clone());
            *n = to;
        }
        Pattern::Aggregate(_, fields, _) => {
            for (_, sub) in fields {
                rename_pattern(sub, tag, renames);
            }
        }
        Pattern::Tuple(items, _) => {
            for i in items {
                rename_pattern(i, tag, renames);
            }
        }
        _ => {}
    }
}

/// Put the arguments where the parameters are.
struct Substitution<'a> {
    bound: &'a HashMap<String, Expr>,
    rest: Option<&'a str>,
    repeated: &'a [Expr],
    span: Span,
    errors: Vec<SyntaxError>,
}

impl Substitution<'_> {
    fn block(&mut self, b: &mut Block) {
        let mut out: Vec<Stmt> = Vec::new();
        for mut s in b.stmts.drain(..) {
            match &mut s {
                Stmt::Let { value, .. } => self.expr(value),
                Stmt::Expr(e) => {
                    self.expr(e);
                    if let Expr::MacroRepeat(stmts, _) = e {
                        out.append(stmts);
                        continue;
                    }
                }
            }
            out.push(s);
        }
        b.stmts = out;
        if let Some(t) = &mut b.tail {
            self.expr(t);
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match e {
            Expr::MacroParam(name, span) => {
                let Some(a) = self.bound.get(name) else {
                    self.errors.push(SyntaxError {
                        span: *span,
                        message: format!("`${name}` is not a parameter of this macro (Ch. 7 §2)"),
                    });
                    return;
                };
                // The argument keeps its own span: it is the caller's text
                // and a diagnostic about it belongs where they wrote it.
                *e = a.clone();
            }
            Expr::MacroRepeat(stmts, span) => {
                let Some(_) = self.rest else {
                    self.errors.push(SyntaxError {
                        span: *span,
                        message: "this macro has no repetition, so `$( … )*` repeats nothing \
                                  (Ch. 7 §3)"
                            .to_string(),
                    });
                    return;
                };
                let template = std::mem::take(stmts);
                let mut out = Vec::new();
                for arg in self.repeated {
                    for s in &template {
                        let mut s = s.clone();
                        let mut one = Substitution {
                            bound: self.bound,
                            rest: None,
                            repeated: &[],
                            span: self.span,
                            errors: Vec::new(),
                        };
                        one.one_repeat(&mut s, arg);
                        self.errors.append(&mut one.errors);
                        out.push(s);
                    }
                }
                *e = Expr::MacroRepeat(out, *span);
            }
            // A block is where a repetition is flattened, so every block
            // has to be reached as one rather than through the generic walk.
            Expr::Block(b) => self.block(b),
            Expr::If(c, then, other, _) => {
                self.expr(c);
                self.block(then);
                if let Some(o) = other {
                    self.expr(o);
                }
            }
            Expr::While(c, b, _) => {
                self.expr(c);
                self.block(b);
            }
            Expr::Loop(b, _) => self.block(b),
            other => walk_children(other, &mut |c| self.expr(c)),
        }
    }

    /// One turn of a repetition: the repeated parameter is this argument,
    /// and every other parameter is what it always was.
    fn one_repeat(&mut self, s: &mut Stmt, arg: &Expr) {
        let go = |e: &mut Expr| replace_param(e, self.bound, arg);
        match s {
            Stmt::Let { value, .. } => go(value),
            Stmt::Expr(e) => go(e),
        }
    }
}

/// Substitute inside one turn of a repetition: any `$name` becomes either a
/// fixed argument or this turn's.
fn replace_param(e: &mut Expr, bound: &HashMap<String, Expr>, arg: &Expr) {
    if let Expr::MacroParam(name, _) = e {
        *e = bound.get(name).cloned().unwrap_or_else(|| arg.clone());
        return;
    }
    walk_children(e, &mut |c| replace_param(c, bound, arg));
}
