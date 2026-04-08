/// A source location with line, column, and length.
///
/// All values are 1-based. A `Span` with `line == 0` is considered a "dummy"
/// span that carries no position information.
///
/// # Examples
///
/// ```
/// use nexa_compiler::domain::span::Span;
///
/// let s = Span::new(15, 12, 3);
/// assert_eq!(s.line, 15);
/// assert_eq!(s.col, 12);
/// assert_eq!(s.len, 3);
/// assert!(!s.is_dummy());
///
/// let d = Span::dummy();
/// assert!(d.is_dummy());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Span {
    /// 1-based line number (0 means "no position information").
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// Character count of the token or construct.
    pub len: u32,
}

impl Span {
    /// Construct a span with explicit line, col, and length.
    pub fn new(line: u32, col: u32, len: u32) -> Self {
        Span { line, col, len }
    }

    /// A sentinel span that carries no position information.
    pub fn dummy() -> Self {
        Span::default()
    }

    /// Returns `true` when this span has no meaningful position data.
    pub fn is_dummy(&self) -> bool {
        self.line == 0
    }
}
