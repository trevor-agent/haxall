// filter/ast.rs — Haystack filter AST types.

use crate::types::Val;

/// A path: one or more tag names joined by "->".
/// `ref->num` = `["ref", "num"]`
#[derive(Debug, Clone, PartialEq)]
pub struct FilterPath {
    pub segments: Vec<String>,
}

impl FilterPath {
    pub fn single(name: impl Into<String>) -> Self {
        FilterPath { segments: vec![name.into()] }
    }

    pub fn multi(names: Vec<String>) -> Self {
        FilterPath { segments: names }
    }

    pub fn first(&self) -> &str {
        &self.segments[0]
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

}

impl std::fmt::Display for FilterPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.segments.join("->"))
    }
}

/// Filter AST node.
#[derive(Debug, Clone)]
pub enum Filter {
    /// Tag is defined (non-null)
    Has(FilterPath),
    /// Tag is not defined (null)
    Missing(FilterPath),
    /// a == b
    Eq(FilterPath, Val),
    /// a != b
    Ne(FilterPath, Val),
    /// a < b
    Lt(FilterPath, Val),
    /// a <= b
    Le(FilterPath, Val),
    /// a > b
    Gt(FilterPath, Val),
    /// a >= b
    Ge(FilterPath, Val),
    /// a and b
    And(Box<Filter>, Box<Filter>),
    /// a or b
    Or(Box<Filter>, Box<Filter>),
    /// isSpec(specName) — requires Xeto schema knowledge; treated as false
    IsSpec(String),
    /// isSymbol(^sym) — treated as false; inner symbol value is not evaluated
    IsSymbol,
}
