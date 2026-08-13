//! Faults — defined machine halts (AM §4).
//!
//! A **fault** is a defined halt with an `F_*` code. It is not UB: UB is the
//! absence of any defined behavior, and the two words are never
//! interchangeable (Naming §4).
//!
//! Faults are not exceptions: the AM has no unwinding (AM §4).
//!
//! This is the complete draft 0.1 code list, exactly as AM §4 tabulates it.

use core::fmt;

/// A defined fault code (AM §4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FaultCode {
    /// `F_OVERFLOW` — trapping-flavor arithmetic overflow (AM §3.1).
    Overflow,
    /// `F_DIVZERO` — division or remainder by zero (AM §3.2).
    DivZero,
    /// `F_SHIFT` — shift amount out of range (AM §3.3). Not masked, not
    /// undefined.
    Shift,
    /// `F_ALIGN` — misaligned access, on targets that check (AM §2.3).
    Align,
    /// `F_TRAP` — explicit trap instruction.
    Trap,
}

impl FaultCode {
    /// Every code, for parser tables and exhaustive tests.
    pub const ALL: [FaultCode; 5] = [
        FaultCode::Overflow,
        FaultCode::DivZero,
        FaultCode::Shift,
        FaultCode::Align,
        FaultCode::Trap,
    ];

    /// The `F_*` spelling used in TIR text.
    pub const fn name(self) -> &'static str {
        match self {
            FaultCode::Overflow => "F_OVERFLOW",
            FaultCode::DivZero => "F_DIVZERO",
            FaultCode::Shift => "F_SHIFT",
            FaultCode::Align => "F_ALIGN",
            FaultCode::Trap => "F_TRAP",
        }
    }

    /// Parse an `F_*` spelling.
    pub fn from_name(s: &str) -> Option<FaultCode> {
        FaultCode::ALL.into_iter().find(|c| c.name() == s)
    }
}

impl fmt::Display for FaultCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A raised fault: a code plus optional context for diagnostics.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fault {
    /// The code.
    pub code: FaultCode,
    /// Human-readable context; not part of the machine semantics.
    pub context: Option<String>,
}

impl Fault {
    /// A fault with no context.
    pub fn new(code: FaultCode) -> Fault {
        Fault {
            code,
            context: None,
        }
    }

    /// A fault carrying diagnostic context.
    pub fn with_context(code: FaultCode, context: impl Into<String>) -> Fault {
        Fault {
            code,
            context: Some(context.into()),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.context {
            Some(c) => write!(f, "fault {}: {c}", self.code),
            None => write!(f, "fault {}", self.code),
        }
    }
}

impl std::error::Error for Fault {}
