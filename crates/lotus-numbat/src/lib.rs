//! Numbat-backed unit / dimensional-analysis handler for lotus-core.
//!
//! Register [`NumbatHandler`] on a [`lotus_core::Registry`] to teach the
//! spreadsheet about quantities: cells like `"3 km"` become
//! `CellValue::Custom` values, and arithmetic / comparisons dispatch
//! through numbat's unit engine. The handler reuses a single
//! [`numbat::Context`] with the prelude loaded (`use prelude`), so
//! SI + imperial + common derived units are available out of the box.
//!
//! ```no_run
//! use std::sync::Arc;
//! use lotus_core::{Registry, Sheet};
//! use lotus_numbat::NumbatHandler;
//!
//! let mut reg = Registry::default();
//! reg.register_type(Arc::new(NumbatHandler::new())).unwrap();
//! let mut sheet = Sheet::new_with_registry(Arc::new(reg));
//! sheet
//!     .set_cells(&[
//!         ("A1".into(), "3 km".into()),
//!         ("A2".into(), "2 mi".into()),
//!         ("A3".into(), "=A1 + A2".into()),
//!     ])
//!     .unwrap();
//! // A3 now holds a `Custom("qty", "...")` with the combined length.
//! ```

use std::sync::{Arc, Mutex};

use lotus_core::{
    BinaryOp, CellValue, CompareOp, CustomFunction, CustomTypeHandler, CustomValue, Registry,
    RegistryError,
};

use numbat::{
    module_importer::BuiltinModuleImporter,
    resolver::CodeSource,
    value::Value,
    Context, InterpreterResult,
};

/// The canonical type tag used by values this handler produces.
pub const TAG: &str = "qty";

/// Register the numbat handler plus the `CONVERT` function on a
/// registry. Both share a single numbat [`Context`] so unit definitions
/// and exchange-rate state (if set) are consistent.
///
/// ```no_run
/// use std::sync::Arc;
/// use lotus_core::{Registry, Sheet};
///
/// let mut reg = Registry::default();
/// lotus_numbat::install(&mut reg).unwrap();
/// let mut sheet = Sheet::new_with_registry(Arc::new(reg));
/// sheet.set_cells(&[
///     ("A1".into(), "100 m/s".into()),
///     ("A2".into(), "=CONVERT(A1, \"mph\")".into()),
/// ]).unwrap();
/// ```
pub fn install(reg: &mut Registry) -> Result<(), RegistryError> {
    let handler = NumbatHandler::new();
    let convert = NumbatConvert {
        ctx: handler.ctx.clone(),
    };
    reg.register_type(Arc::new(handler))?;
    reg.register_function(Arc::new(convert))?;
    Ok(())
}

/// Lotus-core handler that delegates to a shared numbat context.
///
/// Cheap to clone (the context lives behind `Arc<Mutex<_>>`); all
/// handler methods take `&self` and serialize access through the mutex.
#[derive(Clone)]
pub struct NumbatHandler {
    pub(crate) ctx: Arc<Mutex<Context>>,
}

impl Default for NumbatHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NumbatHandler {
    /// Construct a fresh handler with the numbat prelude loaded. Panics
    /// if numbat can't load its built-in modules (shouldn't happen in
    /// practice; the modules are embedded at numbat build time).
    pub fn new() -> Self {
        let mut ctx = Context::new(BuiltinModuleImporter::default());
        let _ = ctx
            .interpret("use prelude", CodeSource::Internal)
            .expect("numbat prelude load");
        NumbatHandler {
            ctx: Arc::new(Mutex::new(ctx)),
        }
    }

    /// Evaluate a raw numbat expression, returning the resulting
    /// quantity's canonical string form. Returns `Err` if numbat
    /// rejects the expression or the result isn't a quantity.
    fn eval_to_quantity_string(&self, code: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().map_err(|_| "numbat ctx poisoned".to_string())?;
        let (_, result) = ctx
            .interpret(code, CodeSource::Text)
            .map_err(|e| format!("#UNIT! {e}"))?;
        match result {
            InterpreterResult::Value(Value::Quantity(q)) => Ok(q.to_string()),
            InterpreterResult::Value(other) => Err(format!("#UNIT! non-quantity result: {other:?}")),
            InterpreterResult::Continue => Err("#UNIT! no value".into()),
        }
    }

    /// Evaluate a numbat boolean expression.
    fn eval_to_bool(&self, code: &str) -> Result<bool, String> {
        let mut ctx = self.ctx.lock().map_err(|_| "numbat ctx poisoned".to_string())?;
        let (_, result) = ctx
            .interpret(code, CodeSource::Text)
            .map_err(|e| format!("#UNIT! {e}"))?;
        match result {
            InterpreterResult::Value(Value::Boolean(b)) => Ok(b),
            _ => Err("#UNIT! non-boolean result".into()),
        }
    }

    /// Render a `CellValue` as a numbat-parseable expression fragment.
    /// Parens are defensive so each side survives its own operator
    /// precedence (e.g. `(2 m) * (3 s)` rather than `2 m * 3 s`).
    fn as_numbat_expr(v: &CellValue) -> Option<String> {
        match v {
            CellValue::Number(_) => Some(format!("({v})")),
            CellValue::Custom(cv) if cv.type_tag == TAG => Some(format!("({})", cv.data)),
            _ => None,
        }
    }
}

impl CustomTypeHandler for NumbatHandler {
    fn type_tag(&self) -> &str {
        TAG
    }

    /// Claim an input iff numbat parses it to a quantity with a
    /// non-dimensionless unit. Bare numbers stay as [`CellValue::Number`].
    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('=') {
            return None;
        }
        // Cheap pre-filter: numbat literals we care about always contain
        // at least one letter (the unit name). Skips bare numbers like
        // "3.14" so they stay as `CellValue::Number` and feed into
        // built-in arithmetic normally.
        if !trimmed.chars().any(|c| c.is_alphabetic()) {
            return None;
        }

        let mut ctx = self.ctx.lock().ok()?;
        let (_, result) = ctx.interpret(trimmed, CodeSource::Text).ok()?;
        let InterpreterResult::Value(Value::Quantity(q)) = result else {
            return None;
        };
        // Only claim truly dimensioned values. A parse that returns a
        // scalar (dimensionless) means numbat saw a bare number through
        // some alias path — don't steal those from `CellValue::Number`.
        if q.unit().is_scalar() {
            return None;
        }
        Some(CustomValue {
            type_tag: TAG.into(),
            data: q.to_string(),
        })
    }

    fn display(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        // `%` and `&` have no meaningful dimensional analog.
        if matches!(op, BinaryOp::Mod | BinaryOp::Concat) {
            return None;
        }
        let l = Self::as_numbat_expr(lhs)?;
        let r = Self::as_numbat_expr(rhs)?;
        let code = format!("{l} {} {r}", op.as_str());
        Some(self.eval_to_quantity_string(&code).map(|data| {
            CellValue::Custom(CustomValue {
                type_tag: TAG.into(),
                data,
            })
        }))
    }

    fn compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        let op_str = match op {
            CompareOp::Eq => "==",
            CompareOp::Ne => "!=",
            CompareOp::Lt => "<",
            CompareOp::Le => "<=",
            CompareOp::Gt => ">",
            CompareOp::Ge => ">=",
        };
        let l = Self::as_numbat_expr(lhs)?;
        let r = Self::as_numbat_expr(rhs)?;
        let code = format!("{l} {op_str} {r}");
        Some(self.eval_to_bool(&code))
    }

    /// Coerce to `f64` via `.as_scalar()` — only succeeds for
    /// dimensionless quantities (e.g. `0.3 * pi`). Dimensioned
    /// quantities can't collapse to a plain number, so `SUM` over a
    /// range of `km` values leaves the engine without a numeric path —
    /// users should instead stay in the binary-op dispatch via
    /// `A1 + A2 + A3` (per-pair).
    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        if v.type_tag != TAG {
            return None;
        }
        let mut ctx = self.ctx.lock().ok()?;
        let (_, result) = ctx.interpret(&v.data, CodeSource::Text).ok()?;
        let InterpreterResult::Value(Value::Quantity(q)) = result else {
            return None;
        };
        q.as_scalar().ok().map(|n| n.to_f64())
    }
}

/// Spreadsheet function that converts a quantity to a target unit:
/// `=CONVERT(A1, "mph")` takes the quantity in `A1` and expresses it
/// in mph. The target unit is any numbat-parseable unit expression
/// (`"km/h"`, `"kg*m/s^2"`, `"hours"`, etc.).
pub struct NumbatConvert {
    pub(crate) ctx: Arc<Mutex<Context>>,
}

impl CustomFunction for NumbatConvert {
    fn name(&self) -> &str {
        "CONVERT"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let value = args
            .first()
            .ok_or_else(|| "#VALUE! CONVERT needs a value".to_string())?;
        let target_unit = match args.get(1) {
            Some(CellValue::String(s)) => s.trim().to_string(),
            _ => return Err("#VALUE! CONVERT needs a unit string as second argument".into()),
        };
        if target_unit.is_empty() {
            return Err("#VALUE! CONVERT: target unit must be non-empty".into());
        }

        let value_expr = match value {
            CellValue::Number(n) => format!("({n})"),
            CellValue::Custom(cv) if cv.type_tag == TAG => format!("({})", cv.data),
            _ => {
                return Err(
                    "#VALUE! CONVERT: first argument must be a numeric or unit value".into(),
                )
            }
        };
        let code = format!("{value_expr} -> {target_unit}");

        let mut ctx = self.ctx.lock().map_err(|_| "numbat ctx poisoned".to_string())?;
        let (_, result) = ctx
            .interpret(&code, CodeSource::Text)
            .map_err(|e| format!("#UNIT! {e}"))?;
        match result {
            InterpreterResult::Value(Value::Quantity(q)) => Ok(CellValue::Custom(CustomValue {
                type_tag: TAG.into(),
                data: q.to_string(),
            })),
            _ => Err("#UNIT! CONVERT: non-quantity result".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lotus_core::{Registry, Sheet};

    fn sheet_with_numbat() -> Sheet {
        let mut reg = Registry::default();
        reg.register_type(Arc::new(NumbatHandler::new())).unwrap();
        Sheet::new_with_registry(Arc::new(reg))
    }

    fn sheet_with_install() -> Sheet {
        let mut reg = Registry::default();
        install(&mut reg).unwrap();
        Sheet::new_with_registry(Arc::new(reg))
    }

    #[test]
    fn literal_km_becomes_custom_qty() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[("A1".into(), "3 km".into())]).unwrap();
        match s.get("A1") {
            CellValue::Custom(cv) => {
                assert_eq!(cv.type_tag, "qty");
                // numbat's canonical form for this is "3 km".
                assert!(cv.data.contains("km"), "got: {}", cv.data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn bare_number_stays_number() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[("A1".into(), "3.14".into())]).unwrap();
        assert_eq!(s.get("A1"), CellValue::Number(3.14));
    }

    #[test]
    fn add_km_and_mi_produces_length() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "3 km".into()),
            ("A2".into(), "2 mi".into()),
            ("A3".into(), "=A1 + A2".into()),
        ])
        .unwrap();
        match s.get("A3") {
            CellValue::Custom(cv) => {
                assert_eq!(cv.type_tag, "qty");
                // Result is a length. Numbat picks a canonical display unit;
                // we just verify it's recognizably a length.
                let d = cv.data.to_lowercase();
                assert!(
                    d.contains("km") || d.contains("m") || d.contains("mi"),
                    "expected length unit in {}",
                    cv.data,
                );
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn dimensional_mismatch_is_an_error() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "3 km".into()),
            ("A2".into(), "2 seconds".into()),
            ("A3".into(), "=A1 + A2".into()),
        ])
        .unwrap();
        // lotus surfaces handler errors as a typed CellValue::Error;
        // the legacy "#UNIT! ..." string lands in FormulaError::Custom.
        match s.get("A3") {
            CellValue::Error(e) => {
                let rendered = e.to_string();
                assert!(rendered.starts_with("#UNIT!"), "got: {rendered}");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn multiply_length_by_scalar() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "5 km".into()),
            ("A2".into(), "=A1 * 3".into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::Custom(cv) => {
                let d = cv.data.to_lowercase();
                assert!(d.contains("km") || d.contains("m"), "got: {}", cv.data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn divide_length_by_time_gives_speed() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "100 km".into()),
            ("A2".into(), "2 hours".into()),
            ("A3".into(), "=A1 / A2".into()),
        ])
        .unwrap();
        match s.get("A3") {
            CellValue::Custom(cv) => {
                // 50 km/h or some canonical velocity unit.
                let d = cv.data.to_lowercase();
                assert!(
                    d.contains("/") || d.contains("per") || d.contains("m/s") || d.contains("km/h"),
                    "expected velocity unit in {}",
                    cv.data,
                );
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn compare_equal_lengths() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "1000 m".into()),
            ("A2".into(), "1 km".into()),
            ("A3".into(), "=A1 = A2".into()),
            ("A4".into(), "=A1 < A2".into()),
        ])
        .unwrap();
        assert_eq!(s.get("A3"), CellValue::Boolean(true));
        assert_eq!(s.get("A4"), CellValue::Boolean(false));
    }

    #[test]
    fn concat_uses_display() {
        let mut s = sheet_with_numbat();
        s.set_cells(&[
            ("A1".into(), "5 km".into()),
            ("A2".into(), r#"="distance: " & A1"#.into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::String(out) => assert!(
                out.starts_with("distance: ") && out.contains("km"),
                "got: {out}"
            ),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn convert_ms_to_mph() {
        let mut s = sheet_with_install();
        s.set_cells(&[
            ("A1".into(), "100 m/s".into()),
            ("A2".into(), "=CONVERT(A1, \"mph\")".into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::Custom(cv) => {
                assert_eq!(cv.type_tag, "qty");
                // 100 m/s ≈ 223.69 mph; we just check the unit is right.
                assert!(cv.data.contains("mph"), "got: {}", cv.data);
                let num: f64 = cv
                    .data
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!((num - 223.69).abs() < 0.1, "got: {}", cv.data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn convert_km_to_miles() {
        let mut s = sheet_with_install();
        s.set_cells(&[
            ("A1".into(), "10 km".into()),
            ("A2".into(), "=CONVERT(A1, \"miles\")".into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::Custom(cv) => {
                assert!(cv.data.contains("mi") || cv.data.contains("mile"), "got: {}", cv.data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn convert_incompatible_dimensions_errors() {
        let mut s = sheet_with_install();
        s.set_cells(&[
            ("A1".into(), "10 km".into()),
            ("A2".into(), "=CONVERT(A1, \"seconds\")".into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::Error(e) => {
                let rendered = e.to_string();
                assert!(rendered.starts_with("#UNIT!"), "got: {rendered}");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn convert_bare_number_with_unit_target() {
        // CONVERT(42, "celsius") is nonsense (42 is dimensionless) → error.
        let mut s = sheet_with_install();
        s.set_cells(&[
            ("A1".into(), "42".into()),
            ("A2".into(), "=CONVERT(A1, \"celsius\")".into()),
        ])
        .unwrap();
        match s.get("A2") {
            CellValue::Error(e) => {
                let rendered = e.to_string();
                assert!(rendered.starts_with("#UNIT!"), "got: {rendered}");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn convert_chain_km_mph() {
        // 100 km/h → mph → km/h: unit round-trip.
        let mut s = sheet_with_install();
        s.set_cells(&[
            ("A1".into(), "100 km/h".into()),
            ("A2".into(), "=CONVERT(A1, \"mph\")".into()),
            ("A3".into(), "=CONVERT(A2, \"km/h\")".into()),
        ])
        .unwrap();
        match s.get("A3") {
            CellValue::Custom(cv) => {
                let num: f64 = cv
                    .data
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!((num - 100.0).abs() < 0.01, "got: {}", cv.data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }
}
