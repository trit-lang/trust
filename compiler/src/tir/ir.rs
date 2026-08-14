//! TIR data structures (TIR §1–§3).
//!
//! Deliberately small, and deliberately not LLVM IR with the numbers changed:
//! one division, one comparison, one select, one conditional terminator, two
//! conversions, no bitcast, no aggregates, opaque pointers.

use trit_core::{Bt, FaultCode, Flavor};

/// The TIR format version this implementation speaks (TIR §8).
///
/// The stamp is a compatibility *check*, not a promise: a module declaring
/// any other version is rejected outright.
pub const TIR_VERSION: &str = "0.1";

/// A TIR type (TIR §2). That is the entire list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Type {
    /// `tN` — balanced ternary integer of exactly N trits.
    Int(u32),
    /// `ptr` — an address, opaque, with no pointee type.
    Ptr,
}

impl Type {
    /// The `tN` width, if this is an integer type.
    pub fn width(self) -> Option<u32> {
        match self {
            Type::Int(n) => Some(n),
            Type::Ptr => None,
        }
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int(n) => write!(f, "t{n}"),
            Type::Ptr => f.write_str("ptr"),
        }
    }
}

/// An instruction operand: an SSA value or an inline constant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Operand {
    /// `%name`.
    Value(String),
    /// `const tN <literal>`.
    Const(Type, Bt),
    /// `@name` — the address of a global, of type `ptr`.
    ///
    /// TIR §1 defines globals but never says how code names one; without an
    /// address form they are unreachable storage. See `docs/spec-gaps.md`.
    Global(String),
}

impl Operand {
    /// The named value, if this operand is one.
    pub fn as_value(&self) -> Option<&str> {
        match self {
            Operand::Value(n) => Some(n),
            Operand::Const(..) | Operand::Global(_) => None,
        }
    }
}

/// A branch destination plus the arguments for its block parameters.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    /// `^label`.
    pub label: String,
    /// Arguments, matching the destination block's parameter list.
    pub args: Vec<Operand>,
}

/// A binary arithmetic operation that carries an overflow flavor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlavoredOp {
    /// `add`.
    Add,
    /// `sub`.
    Sub,
    /// `mul`.
    Mul,
    /// `shl` — `%a * 3^%k`.
    Shl,
}

impl FlavoredOp {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        match self {
            FlavoredOp::Add => "add",
            FlavoredOp::Sub => "sub",
            FlavoredOp::Mul => "mul",
            FlavoredOp::Shl => "shl",
        }
    }
}

/// A binary operation that is total (or faults on its own terms) and so needs
/// no flavor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlainOp {
    /// `mulh` — the high tN of the exact 2N-trit product, so that
    /// `mulh · 3^N + mul.wrap` reconstructs it (TIR §3.1). It cannot
    /// overflow, which is why it is here and not among the flavored ops.
    MulH,
    /// `div` — round to nearest, ties away from zero. The only division.
    Div,
    /// `rem` — `|r| ≤ |b|/2`.
    Rem,
    /// `shr` — `%a / 3^%k`, exact.
    Shr,
    /// `tmin`.
    TMin,
    /// `tmax`.
    TMax,
    /// `tmul`.
    TMul,
}

impl PlainOp {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        match self {
            PlainOp::MulH => "mulh",
            PlainOp::Div => "div",
            PlainOp::Rem => "rem",
            PlainOp::Shr => "shr",
            PlainOp::TMin => "tmin",
            PlainOp::TMax => "tmax",
            PlainOp::TMul => "tmul",
        }
    }
}

/// What an instruction computes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InstKind {
    /// `add<fl> tN %a, %b` and friends. Yields one result, or two under
    /// `.flag` (wrapped value, overflow trit).
    Flavored {
        /// Which operation.
        op: FlavoredOp,
        /// Overflow flavor.
        flavor: Flavor,
        /// Operand width.
        ty: Type,
        /// Left operand.
        a: Operand,
        /// Right operand.
        b: Operand,
    },
    /// `div`/`rem`/`shr`/`tmin`/`tmax`/`tmul`.
    Plain {
        /// Which operation.
        op: PlainOp,
        /// Operand width.
        ty: Type,
        /// Left operand.
        a: Operand,
        /// Right operand.
        b: Operand,
    },
    /// `neg tN %a` — total, no flavor. `tneg` parses to this: "alias of neg —
    /// one canonical form after parsing" (TIR §3.2).
    Neg {
        /// Operand width.
        ty: Type,
        /// The operand.
        a: Operand,
    },
    /// `cmp tN %a, %b -> t1` — three-way, the only comparison.
    Cmp {
        /// Operand width.
        ty: Type,
        /// Left operand.
        a: Operand,
        /// Right operand.
        b: Operand,
    },
    /// `select3 %t, tN %vn, %vz, %vp`.
    Select3 {
        /// The `t1` selector.
        t: Operand,
        /// Result width.
        ty: Type,
        /// Value chosen when the selector is −1.
        neg: Operand,
        /// Value chosen when the selector is 0.
        zero: Operand,
        /// Value chosen when the selector is +1.
        pos: Operand,
    },
    /// `slot tryte[N]` — stack allocation of function lifetime, yields `ptr`.
    Slot {
        /// Size in trytes.
        trytes: u32,
    },
    /// `load tN %p`.
    Load {
        /// Accessed type.
        ty: Type,
        /// Address.
        p: Operand,
    },
    /// `store tN %v, %p`.
    Store {
        /// Accessed type.
        ty: Type,
        /// Stored value.
        v: Operand,
        /// Address.
        p: Operand,
    },
    /// `offset %p, %d` — `ptr` + d trytes. The entire address arithmetic.
    Offset {
        /// Base address.
        p: Operand,
        /// Displacement in trytes.
        d: Operand,
    },
    /// `widen tM %a -> tN` — value-preserving.
    Widen {
        /// Source width.
        from: Type,
        /// The operand.
        a: Operand,
        /// Destination width.
        to: Type,
    },
    /// `trunc tN %a -> tM` — wraps into the narrow symmetric range.
    Trunc {
        /// Source width.
        from: Type,
        /// The operand.
        a: Operand,
        /// Destination width.
        to: Type,
    },
    /// `call @f(%a, %b) -> tN`.
    Call {
        /// Callee: a symbol for a direct call, or a `ptr` operand for an
        /// indirect one (TIR §3.7).
        callee: Callee,
        /// Arguments.
        args: Vec<Operand>,
        /// Return type, absent for `()` functions.
        ret: Option<Type>,
    },
}

/// One instruction: its results and what it computes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inst {
    /// Result names — empty for `store`, two for `.flag`, else one.
    pub results: Vec<String>,
    /// The operation.
    pub kind: InstKind,
}

/// A block terminator (TIR §3.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Terminator {
    /// `br3 %t, ^neg(...), ^zero(...), ^pos(...)` — the only conditional
    /// terminator.
    Br3 {
        /// The `t1` selector.
        t: Operand,
        /// Destination when the selector is −1.
        neg: Target,
        /// Destination when the selector is 0.
        zero: Target,
        /// Destination when the selector is +1.
        pos: Target,
    },
    /// `br ^dest(...)`.
    Br(Target),
    /// `ret %v`, or `ret` for `()` functions.
    Ret(Option<Operand>),
    /// `trap F_CODE` — a deliberate fault.
    Trap(FaultCode),
    /// `unreachable` — a UB marker.
    Unreachable,
}

/// A basic block: parameters, instructions, exactly one terminator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Block {
    /// `^label`.
    pub label: String,
    /// Block parameters — the phi-equivalent (TIR §1.1).
    pub params: Vec<(String, Type)>,
    /// Instructions, in order.
    pub insts: Vec<Inst>,
    /// The terminator.
    pub term: Terminator,
}

/// A function signature.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Signature {
    /// `@name`.
    pub name: String,
    /// Parameters, named because the entry block takes them directly.
    pub params: Vec<(String, Type)>,
    /// Return type, absent for `()` functions.
    pub ret: Option<Type>,
}

/// A function definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Function {
    /// The signature.
    pub sig: Signature,
    /// Blocks; the first is the entry block and may not be a branch target.
    pub blocks: Vec<Block>,
}

impl Function {
    /// The block with the given label.
    pub fn block(&self, label: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.label == label)
    }
}

/// A global definition: `global @name : tryte[N] = <initializer>`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Global {
    /// `@name`.
    pub name: String,
    /// Size in trytes.
    pub trytes: u32,
    /// Initializer items, from the lowest address (little-trytean, AM §2.2).
    /// Absent means zero-initialized. (TIR §1.2.)
    pub init: Option<Vec<InitItem>>,
}

/// What a `call` transfers to (TIR §3.7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    /// `call @f(…)` — a module-scope symbol.
    Direct(String),
    /// `call %p(…)` — the function whose address the pointer holds.
    Indirect(Operand),
}

/// One item of a global initializer (TIR §1.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InitItem {
    /// One tryte of data.
    Tryte(Bt),
    /// The address of a module-scope symbol, one word wide. Its value is not
    /// known until the module is placed, so it is a relocation: TIR says
    /// which symbol and the target decides the number.
    Addr(String),
}

impl InitItem {
    /// How many trytes it fills.
    pub fn trytes(&self) -> u32 {
        match self {
            InitItem::Tryte(_) => 1,
            // A word, which is what a `ptr` is (TIR §2).
            InitItem::Addr(_) => 3,
        }
    }
}

/// A TIR module.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Module {
    /// The format version stamp from the header.
    pub version: String,
    /// The target name from the header.
    pub target: String,
    /// Global definitions.
    pub globals: Vec<Global>,
    /// Declarations — signature only, body external.
    pub decls: Vec<Signature>,
    /// Definitions.
    pub funcs: Vec<Function>,
}

impl Module {
    /// The definition of `@name`, if this module defines it.
    pub fn function(&self, name: &str) -> Option<&Function> {
        self.funcs.iter().find(|f| f.sig.name == name)
    }

    /// The signature of `@name`, whether declared or defined.
    pub fn signature(&self, name: &str) -> Option<&Signature> {
        self.funcs
            .iter()
            .map(|f| &f.sig)
            .chain(self.decls.iter())
            .find(|s| s.name == name)
    }
}
