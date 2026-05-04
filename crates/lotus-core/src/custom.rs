use std::sync::Arc;

use crate::types::{BinaryOp, CellValue, CompareOp, CustomValue};

/// Runtime hook for teaching the engine about a new value type.
///
/// A single handler owns a `type_tag` (e.g. `"qty"`, `"polygon"`, `"pdf"`).
/// Runtimes register one handler per type via [`Registry::register_type`].
/// Every method except `type_tag` has a default no-op impl, so handlers
/// only implement what they need.
pub trait CustomTypeHandler: Send + Sync {
    fn type_tag(&self) -> &str;

    /// Try to parse a raw cell input string into a [`CustomValue`].
    ///
    /// Called at ingestion time (before numeric / string fallback). Return
    /// `None` to decline — the next handler, then numeric coercion, then
    /// string fallback, will be tried in order.
    fn parse_literal(&self, _raw: &str) -> Option<CustomValue> {
        None
    }

    /// Try to parse `raw` using handler-specific `options`. The string is
    /// opaque to the engine — the handler defines its grammar (often a
    /// strftime pattern, JSON blob, etc.). Reached only via the explicit
    /// [`Registry::try_parse`] / [`crate::Sheet::try_parse`] probe; the
    /// normal ingestion pipeline still goes through `parse_literal`.
    ///
    /// Default: ignore `options` and delegate to `parse_literal`, so an
    /// empty options string mirrors today's behaviour.
    fn parse_with(&self, raw: &str, _options: &str) -> Option<CustomValue> {
        self.parse_literal(raw)
    }

    /// User-facing rendering (grid cell text, `CONCAT` output, etc.).
    fn display(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    /// Edit-buffer string (what the editor shows when the user opens a
    /// cell for editing). Runtimes may surface this via their own UI.
    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    /// Dispatch for `+ - * / ^ % &`. Either operand may be any
    /// [`CellValue`] variant (Number, String, Empty, Custom). Return
    /// `None` to decline — the engine falls through to its default
    /// numeric / string behavior.
    fn binary_op(
        &self,
        _op: BinaryOp,
        _lhs: &CellValue,
        _rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        None
    }

    /// Dispatch for `= <> < <= > >=`. Same contract as `binary_op`.
    fn compare(
        &self,
        _op: CompareOp,
        _lhs: &CellValue,
        _rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        None
    }

    /// Coerce a custom value to `f64` for built-in aggregate functions
    /// (`SUM` / `AVERAGE` / `MIN` / `MAX`). Return `None` to be skipped
    /// (matching how non-numeric strings are skipped today).
    fn as_number(&self, _v: &CustomValue) -> Option<f64> {
        None
    }
}

/// Runtime hook for a named spreadsheet function. Receives args
/// pre-flattened from any range / array inputs (matching the built-in
/// aggregate dispatch).
pub trait CustomFunction: Send + Sync {
    fn name(&self) -> &str;
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String>;
}

/// Registration failure. Collisions are detected at register-time so that
/// a sheet can never get into a state with ambiguous dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// A handler with this `type_tag` is already registered.
    DuplicateTypeTag(String),
    /// A function with this name (or alias) is already registered as a
    /// custom function or as a built-in.
    DuplicateFunction(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::DuplicateTypeTag(t) => {
                write!(f, "custom type tag already registered: {t}")
            }
            RegistryError::DuplicateFunction(n) => {
                write!(f, "custom function name collides with existing: {n}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Set of custom types + functions a [`crate::Sheet`] or [`crate::Evaluator`]
/// will use. Clone is cheap: each entry is an `Arc`.
#[derive(Default, Clone)]
pub struct Registry {
    types: Vec<Arc<dyn CustomTypeHandler>>,
    functions: Vec<Arc<dyn CustomFunction>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let type_tags: Vec<&str> = self.types.iter().map(|h| h.type_tag()).collect();
        let fn_names: Vec<&str> = self.functions.iter().map(|h| h.name()).collect();
        f.debug_struct("Registry")
            .field("types", &type_tags)
            .field("functions", &fn_names)
            .finish()
    }
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a custom type handler. Errors on duplicate `type_tag`.
    pub fn register_type(
        &mut self,
        handler: Arc<dyn CustomTypeHandler>,
    ) -> Result<(), RegistryError> {
        let tag = handler.type_tag().to_string();
        if self.types.iter().any(|h| h.type_tag() == tag) {
            return Err(RegistryError::DuplicateTypeTag(tag));
        }
        self.types.push(handler);
        Ok(())
    }

    /// Register a custom function. Errors on collision with a built-in or
    /// previously-registered custom function (case-insensitive).
    pub fn register_function(
        &mut self,
        func: Arc<dyn CustomFunction>,
    ) -> Result<(), RegistryError> {
        let name = func.name().to_string();
        let upper = name.to_uppercase();
        if crate::functions::is_builtin_function(&upper)
            || self
                .functions
                .iter()
                .any(|f| f.name().eq_ignore_ascii_case(&name))
        {
            return Err(RegistryError::DuplicateFunction(name));
        }
        self.functions.push(func);
        Ok(())
    }

    /// Try each handler's `parse_literal` in registration order, stopping
    /// at the first match.
    pub fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        for h in &self.types {
            if let Some(v) = h.parse_literal(raw) {
                return Some(v);
            }
        }
        None
    }

    /// Probe a specific handler with handler-defined `options` (e.g. a
    /// strftime pattern for a date handler). Returns `None` if no
    /// handler is registered for `type_tag` or the handler declines.
    /// Embedders use this to ask "can `raw` be parsed as `type_tag`
    /// with these options?" before deciding what to write into a cell.
    pub fn try_parse(
        &self,
        type_tag: &str,
        raw: &str,
        options: &str,
    ) -> Option<CustomValue> {
        self.handler_for(type_tag)
            .and_then(|h| h.parse_with(raw, options))
    }

    /// Render a custom value via its registered handler, or fall back to
    /// the raw `data` string if no handler is registered for the tag.
    pub fn display(&self, v: &CustomValue) -> String {
        self.handler_for(&v.type_tag)
            .map(|h| h.display(v))
            .unwrap_or_else(|| v.data.clone())
    }

    /// Editor-buffer repr (see [`CustomTypeHandler::edit_repr`]).
    pub fn edit_repr(&self, v: &CustomValue) -> String {
        self.handler_for(&v.type_tag)
            .map(|h| h.edit_repr(v))
            .unwrap_or_else(|| v.data.clone())
    }

    /// Numeric coercion for a single custom value (see
    /// [`CustomTypeHandler::as_number`]).
    pub fn as_number(&self, v: &CustomValue) -> Option<f64> {
        self.handler_for(&v.type_tag).and_then(|h| h.as_number(v))
    }

    /// User-facing stringification for any [`CellValue`]. Custom values
    /// go through the handler's `display`; everything else uses the
    /// normal `Display` impl.
    pub fn render(&self, v: &CellValue) -> String {
        match v {
            CellValue::Custom(cv) => self.display(cv),
            _ => v.to_string(),
        }
    }

    /// Registry-aware numeric coercion for any [`CellValue`]. Custom
    /// values go through the handler's `as_number`; everything else
    /// through [`CellValue::as_number`].
    pub fn coerce_number(&self, v: &CellValue) -> Option<f64> {
        match v {
            CellValue::Custom(cv) => self.as_number(cv),
            _ => v.as_number(),
        }
    }

    /// Dispatch a binary op through the registry. Tries handlers in
    /// registration order; the first `Some` wins. Returns `None` if no
    /// handler claims the operation (engine should fall through to its
    /// default behavior).
    pub fn dispatch_binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, crate::FormulaError>> {
        for h in &self.types {
            if let Some(result) = h.binary_op(op, lhs, rhs) {
                return Some(result.map_err(crate::FormulaError::from));
            }
        }
        None
    }

    /// Dispatch a comparison through the registry. Same rules as
    /// `dispatch_binary_op`.
    pub fn dispatch_compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, crate::FormulaError>> {
        for h in &self.types {
            if let Some(result) = h.compare(op, lhs, rhs) {
                return Some(result.map_err(crate::FormulaError::from));
            }
        }
        None
    }

    /// Look up a registered function by name (case-insensitive).
    /// Returns `None` immediately when no functions are registered,
    /// which is the common case on every built-in formula call.
    pub fn lookup_function(&self, name: &str) -> Option<&Arc<dyn CustomFunction>> {
        if self.functions.is_empty() {
            return None;
        }
        self.functions
            .iter()
            .find(|f| f.name().eq_ignore_ascii_case(name))
    }

    /// Iterate registered functions in registration order. Used by the
    /// completion / signature-help surface to enumerate non-builtin
    /// names alongside the static builtin table.
    pub fn iter_functions(&self) -> impl Iterator<Item = &dyn CustomFunction> {
        self.functions.iter().map(|f| f.as_ref())
    }

    fn handler_for(&self, type_tag: &str) -> Option<&Arc<dyn CustomTypeHandler>> {
        self.types.iter().find(|h| h.type_tag() == type_tag)
    }

    /// Consume the registry and return its function vector. Used by
    /// extension crates that build a `Registry` for collision-detection
    /// purposes and then need to drain the functions onto a `Sheet`
    /// that's already been constructed.
    #[must_use]
    pub fn into_functions(self) -> Vec<Arc<dyn CustomFunction>> {
        self.functions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TagOnly(&'static str);
    impl CustomTypeHandler for TagOnly {
        fn type_tag(&self) -> &str {
            self.0
        }
    }

    struct NamedFn(&'static str);
    impl CustomFunction for NamedFn {
        fn name(&self) -> &str {
            self.0
        }
        fn call(&self, _args: &[CellValue]) -> Result<CellValue, String> {
            Ok(CellValue::Empty)
        }
    }

    #[test]
    fn register_type_rejects_duplicate_tag() {
        let mut reg = Registry::new();
        reg.register_type(Arc::new(TagOnly("qty"))).unwrap();
        let err = reg.register_type(Arc::new(TagOnly("qty"))).unwrap_err();
        assert_eq!(err, RegistryError::DuplicateTypeTag("qty".into()));
    }

    #[test]
    fn register_function_rejects_builtin_name() {
        let mut reg = Registry::new();
        let err = reg.register_function(Arc::new(NamedFn("SUM"))).unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateFunction(_)));
    }

    #[test]
    fn register_function_rejects_duplicate_custom() {
        let mut reg = Registry::new();
        reg.register_function(Arc::new(NamedFn("SHOUT"))).unwrap();
        let err = reg
            .register_function(Arc::new(NamedFn("shout")))
            .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateFunction(_)));
    }
}
