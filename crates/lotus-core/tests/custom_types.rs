//! End-to-end test that a runtime-supplied `CustomTypeHandler` +
//! `CustomFunction` are picked up by all four engine dispatch sites:
//! literal parse, binary op, compare, function call.
//!
//! The dummy type is `!upper:...` — anything a user types with that
//! prefix becomes an `UpperString` custom value, which:
//!   - displays as its upper-cased tail
//!   - binary-op `+` concatenates two UpperStrings and upper-cases
//!   - binary-op `&` is left to the engine (fall-through test)
//!   - `=` / `<>` compare via `data` equality (via handler)
//!
//! The registered function `SHOUT` wraps a scalar as an UpperString.
//!
//! This is intentionally a silly domain — the point is to prove the
//! extension surface works end-to-end, not to ship a useful custom type.

use std::sync::Arc;

use lotus_core::{
    BinaryOp, CellInput, CellValue, CompareOp, CustomFunction, CustomTypeHandler, CustomValue,
    Registry, Sheet,
};

struct UpperHandler;

impl CustomTypeHandler for UpperHandler {
    fn type_tag(&self) -> &str {
        "upper"
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        raw.strip_prefix("!upper:").map(|rest| CustomValue {
            type_tag: "upper".into(),
            data: rest.to_uppercase(),
        })
    }

    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        // Probe API: when `options == "force"` the handler claims any
        // input (no `!upper:` prefix needed). Empty options matches the
        // ingestion-pipeline behaviour exactly.
        if options == "force" {
            return Some(CustomValue {
                type_tag: "upper".into(),
                data: raw.to_uppercase(),
            });
        }
        self.parse_literal(raw)
    }

    fn display(&self, v: &CustomValue) -> String {
        format!("⟨{}⟩", v.data)
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        if op != BinaryOp::Add {
            return None;
        }
        match (lhs, rhs) {
            (CellValue::Custom(a), CellValue::Custom(b))
                if a.type_tag == "upper" && b.type_tag == "upper" =>
            {
                Some(Ok(CellValue::Custom(CustomValue {
                    type_tag: "upper".into(),
                    data: format!("{}{}", a.data, b.data),
                })))
            }
            _ => None,
        }
    }

    fn compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        match (lhs, rhs) {
            (CellValue::Custom(a), CellValue::Custom(b))
                if a.type_tag == "upper" && b.type_tag == "upper" =>
            {
                Some(Ok(match op {
                    CompareOp::Eq => a.data == b.data,
                    CompareOp::Ne => a.data != b.data,
                    _ => false,
                }))
            }
            _ => None,
        }
    }
}

struct ShoutFn;

impl CustomFunction for ShoutFn {
    fn name(&self) -> &str {
        "SHOUT"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let s = args
            .first()
            .map(|v| v.to_string())
            .unwrap_or_default();
        Ok(CellValue::Custom(CustomValue {
            type_tag: "upper".into(),
            data: s.to_uppercase(),
        }))
    }
}

fn registry() -> Arc<Registry> {
    let mut reg = Registry::default();
    reg.register_type(Arc::new(UpperHandler)).unwrap();
    reg.register_function(Arc::new(ShoutFn)).unwrap();
    Arc::new(reg)
}

fn sheet() -> Sheet {
    Sheet::new_with_registry(registry())
}

#[test]
fn ingestion_routes_literal_to_handler() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "!upper:hello".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => {
            assert_eq!(cv.type_tag, "upper");
            assert_eq!(cv.data, "HELLO");
        }
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn binary_op_dispatches_to_handler() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "!upper:hello ".into()),
        ("A2".into(), "!upper:world".into()),
        ("A3".into(), "=A1+A2".into()),
    ])
    .unwrap();
    match s.get("A3") {
        CellValue::Custom(cv) => assert_eq!(cv.data, "HELLO WORLD"),
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn binary_op_declined_falls_through() {
    // `*` isn't handled by the handler → engine default → Empty (no numeric
    // coercion available for Customs with no `as_number`).
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "!upper:x".into()),
        ("A2".into(), "!upper:y".into()),
        ("A3".into(), "=A1*A2".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Empty);
}

#[test]
fn concat_operator_uses_registry_display() {
    // `&` is NOT intercepted by the handler (returns None for that op),
    // so the engine's default `&` runs and must use `registry.display()`
    // for the Custom side — yielding `⟨FOO⟩`, not `FOO`.
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "!upper:foo".into()),
        ("A2".into(), r#"="pre-" & A1"#.into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), CellValue::String("pre-⟨FOO⟩".into()));
}

#[test]
fn compare_dispatches_to_handler() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "!upper:same".into()),
        ("A2".into(), "!upper:same".into()),
        ("A3".into(), "!upper:diff".into()),
        ("B1".into(), "=A1=A2".into()), // true
        ("B2".into(), "=A1=A3".into()), // false
        ("B3".into(), "=A1<>A3".into()), // true
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    assert_eq!(s.get("B2"), CellValue::Boolean(false));
    assert_eq!(s.get("B3"), CellValue::Boolean(true));
}

#[test]
fn custom_function_call() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "hello".into()),
        ("A2".into(), "=SHOUT(A1)".into()),
    ])
    .unwrap();
    match s.get("A2") {
        CellValue::Custom(cv) => {
            assert_eq!(cv.type_tag, "upper");
            assert_eq!(cv.data, "HELLO");
        }
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn custom_function_result_feeds_back_through_handler() {
    // SHOUT produces a Custom; CONCAT should then render it via display().
    let mut s = sheet();
    s.set_cells(&[("A1".into(), r#"=CONCAT("greeting: ", SHOUT("hi"))"#.into())])
        .unwrap();
    assert_eq!(s.get("A1"), CellValue::String("greeting: ⟨HI⟩".into()));
}

#[test]
fn register_collision_with_builtin_rejected() {
    let mut reg = Registry::default();
    let err = reg
        .register_function(Arc::new(ShoutFnNamed("SUM")))
        .unwrap_err();
    assert!(matches!(
        err,
        lotus_core::RegistryError::DuplicateFunction(_)
    ));
}

#[test]
fn registry_default_leaves_engine_unchanged() {
    // Same formulas through a sheet with NO registry should behave
    // exactly as before. (Regression guard.)
    let mut s = Sheet::new();
    s.set_cells(&[
        ("A1".into(), "10".into()),
        ("A2".into(), "=A1*2".into()),
        ("A3".into(), "!upper:x".into()), // just a string now
    ])
    .unwrap();
    assert_eq!(s.get("A2"), CellValue::Number(20.0));
    assert_eq!(s.get("A3"), CellValue::String("!upper:x".into()));
}

#[test]
fn try_parse_probes_handler_with_options() {
    // Empty options ⇒ same gate as parse_literal (needs `!upper:` prefix).
    let s = sheet();
    assert!(s.try_parse("upper", "hello", "").is_none());
    let cv = s.try_parse("upper", "!upper:hi", "").unwrap();
    assert_eq!(cv.data, "HI");

    // Handler-specific option: `force` makes the handler claim any input.
    let cv = s.try_parse("upper", "hello", "force").unwrap();
    assert_eq!(cv.type_tag, "upper");
    assert_eq!(cv.data, "HELLO");
}

#[test]
fn try_parse_unknown_type_tag_returns_none() {
    let s = sheet();
    assert!(s.try_parse("does-not-exist", "anything", "").is_none());
}

#[test]
fn typed_input_bypasses_parse_pipeline() {
    // Even though "!upper:hi" would normally route through the handler,
    // a Typed write stores whatever CellValue the embedder supplied.
    let mut s = sheet();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::String("!upper:hi".into())),
    )])
    .unwrap();
    assert_eq!(s.get("A1"), CellValue::String("!upper:hi".into()));
}

#[test]
fn typed_input_preserves_string_against_numeric_coercion() {
    // Without the bypass, "2/4" parses as 0.5 (f64). Typed forces String.
    let mut s = Sheet::new();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::String("2/4".into())),
    )])
    .unwrap();
    assert_eq!(s.get("A1"), CellValue::String("2/4".into()));
}

#[test]
fn typed_input_visible_to_dependent_formula() {
    // A formula in B1 reads A1's typed value through the normal resolver.
    let mut s = sheet();
    s.set_cells_typed(&[
        (
            "A1".into(),
            CellInput::Typed(CellValue::Custom(CustomValue {
                type_tag: "upper".into(),
                data: "HELLO".into(),
            })),
        ),
        (
            "A2".into(),
            CellInput::Typed(CellValue::Custom(CustomValue {
                type_tag: "upper".into(),
                data: " WORLD".into(),
            })),
        ),
        ("B1".into(), CellInput::Raw("=A1+A2".into())),
    ])
    .unwrap();
    match s.get("B1") {
        CellValue::Custom(cv) => assert_eq!(cv.data, "HELLO WORLD"),
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn raw_write_clears_prior_typed_override() {
    // Same input, two different intents: "42" stored as a forced String
    // first, then opted back into auto-classification with a Raw write.
    let mut s = Sheet::new();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::String("42".into())),
    )])
    .unwrap();
    assert_eq!(s.get("A1"), CellValue::String("42".into()));

    // Raw write drops the override → engine re-parses "42" as a number.
    s.set_cells(&[("A1".into(), "42".into())]).unwrap();
    assert_eq!(s.get("A1"), CellValue::Number(42.0));
}

#[test]
fn typed_raw_starts_with_equals_does_not_become_formula() {
    // Render of CellValue::String("=foo") is "=foo", which would normally
    // be parsed as a formula. The typed override must short-circuit that.
    let mut s = Sheet::new();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::String("=foo".into())),
    )])
    .unwrap();
    assert_eq!(s.get("A1"), CellValue::String("=foo".into()));
}

#[test]
fn typed_raw_renders_via_registry() {
    // For Custom values raw should be the registry-rendered display
    // (UpperHandler::display wraps in ⟨...⟩), giving editors a faithful
    // round-trip representation.
    let mut s = sheet();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::Custom(CustomValue {
            type_tag: "upper".into(),
            data: "HELLO".into(),
        })),
    )])
    .unwrap();
    assert_eq!(s.get_raw("A1"), Some("⟨HELLO⟩"));
}

#[test]
fn empty_raw_clears_typed_cell() {
    let mut s = Sheet::new();
    s.set_cells_typed(&[(
        "A1".into(),
        CellInput::Typed(CellValue::String("hello".into())),
    )])
    .unwrap();
    s.set_cells(&[("A1".into(), String::new())]).unwrap();
    assert_eq!(s.get("A1"), CellValue::Empty);
    assert_eq!(s.get_raw("A1"), None);
}

// Helper for the collision test: a CustomFunction with a caller-chosen name.
struct ShoutFnNamed(&'static str);
impl CustomFunction for ShoutFnNamed {
    fn name(&self) -> &str {
        self.0
    }
    fn call(&self, _args: &[CellValue]) -> Result<CellValue, String> {
        Ok(CellValue::Empty)
    }
}
