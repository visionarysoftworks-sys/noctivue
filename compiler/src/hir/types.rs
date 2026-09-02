//! HIR type representation — the resolved, normalised view of all types.
//!
//! `Ty` is the canonical type after lowering from `ast::TypeExpr`.  It is used
//! throughout the HIR and type-checker.  Monomorphised generics remain as
//! `Named` until a later lowering pass (out of scope for M0).

/// A resolved type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    // ── Primitives ─────────────────────────────────────────────────────────
    Int,
    UInt,
    Float,
    Bool,
    Char,
    String,
    /// The empty / void type `()`.
    Unit,

    // ── Nominal / generic ──────────────────────────────────────────────────
    /// A named type (struct, enum, alias, or unresolved generic).  Generic
    /// arguments are preserved but not monomorphised in M0.
    Named(std::string::String, Vec<Ty>),

    // ── Standard wrappers ─────────────────────────────────────────────────
    /// `Option<T>` — absence without null.
    Option(Box<Ty>),
    /// `Result<T, E>` — fallible computation.
    Result(Box<Ty>, Box<Ty>),

    // ── Compound ──────────────────────────────────────────────────────────
    /// `[T]` — growable list / array.
    List(Box<Ty>),
    /// `(A, B, …)` — tuple.
    Tuple(Vec<Ty>),
    /// `(A, B) -> C` — function type.
    Fn(Vec<Ty>, Box<Ty>),

    // ── Inference sentinels ───────────────────────────────────────────────
    /// Placeholder during inference — type not yet determined.
    Unknown,
    /// Propagation sentinel — a type error was already reported for this
    /// expression, so downstream errors on this value should be suppressed.
    Error,
}

impl Ty {
    /// Returns `true` if this type is `Result<_, _>` or `Option<_>`, meaning
    /// the `?` operator may be applied to it.
    pub fn is_fallible(&self) -> bool {
        matches!(self, Ty::Result(_, _) | Ty::Option(_))
    }

    /// If `self` is `Result<T, E>`, return `T`.  If `Option<T>`, return `T`.
    /// Returns `None` otherwise.
    pub fn unwrap_fallible(&self) -> Option<Ty> {
        match self {
            Ty::Result(ok, _) => Some(*ok.clone()),
            Ty::Option(inner) => Some(*inner.clone()),
            _ => None,
        }
    }

    /// Return `true` when this type is the `Error` sentinel.
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }

    /// Return `true` when this type is `Unknown`.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    /// Structural equality check that treats `Error` and `Unknown` as
    /// compatible with anything (so we don't cascade-report after the first
    /// error).
    pub fn compatible_with(&self, other: &Ty) -> bool {
        if self.is_error() || other.is_error() {
            return true;
        }
        if self.is_unknown() || other.is_unknown() {
            return true;
        }
        // Recurse into wrapper types so that partial inference (Unknown in a
        // type argument) is accepted rather than triggering a cascade.
        match (self, other) {
            (Ty::Option(a), Ty::Option(b)) => a.compatible_with(b),
            (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err)) => {
                a_ok.compatible_with(b_ok) && a_err.compatible_with(b_err)
            }
            (Ty::List(a), Ty::List(b)) => a.compatible_with(b),
            (Ty::Tuple(a), Ty::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.compatible_with(y))
            }
            (Ty::Fn(ap, ar), Ty::Fn(bp, br)) => {
                ap.len() == bp.len()
                    && ap.iter().zip(bp.iter()).all(|(x, y)| x.compatible_with(y))
                    && ar.compatible_with(br)
            }
            _ => self == other,
        }
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::UInt => write!(f, "UInt"),
            Ty::Float => write!(f, "Float"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Char => write!(f, "Char"),
            Ty::String => write!(f, "String"),
            Ty::Unit => write!(f, "()"),
            Ty::Named(name, args) if args.is_empty() => write!(f, "{name}"),
            Ty::Named(name, args) => {
                write!(f, "{name}<")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ">")
            }
            Ty::Option(inner) => write!(f, "Option<{inner}>"),
            Ty::Result(ok, err) => write!(f, "Result<{ok}, {err}>"),
            Ty::List(inner) => write!(f, "[{inner}]"),
            Ty::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e}")?;
                }
                write!(f, ")")
            }
            Ty::Fn(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret}")
            }
            Ty::Unknown => write!(f, "?"),
            Ty::Error => write!(f, "<error>"),
        }
    }
}
