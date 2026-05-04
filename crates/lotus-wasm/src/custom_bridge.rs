//! Trampoline types that wrap JS objects / functions and present them
//! to `lotus-core` as `CustomTypeHandler` / `CustomFunction` impls.
//!
//! ## JS handler contract
//!
//! A handler registered via `WasmSheet::register_type(obj)` is a plain
//! object with these optional methods (all take / return plain JS
//! values — no wrapper classes):
//!
//! ```js
//! {
//!   typeTag: "polygon",                       // required
//!   parseLiteral(raw: string): {data:string}|null,
//!   display(v: {type_tag, data}): string,
//!   editRepr(v): string,
//!   binaryOp(op: "+"|"-"|"*"|"/"|"^"|"%"|"&", lhs, rhs): <CellValue>|{error:string}|null,
//!   compare(op: "="|"<>"|"<"|"<="|">"|">=", lhs, rhs): boolean|{error:string}|null,
//!   asNumber(v): number|null,
//! }
//! ```
//!
//! where `<CellValue>` across the boundary is one of:
//!   - `number`
//!   - `string`
//!   - `null` (empty)
//!   - `{ type_tag: string, data: string }` (custom)
//!
//! A JS function registered via `register_function(name, fn)` receives
//! the pre-flattened arg array and returns a CellValue:
//!
//! ```js
//! wb.register_function("AREA", (args) => turf.area(JSON.parse(args[0].data)))
//! ```

use lotus_core::{
    BinaryOp, CellValue, CompareOp, CustomFunction, CustomTypeHandler, CustomValue,
};
use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;

/// JS method names looked up on a handler object at registration time.
const METHOD_NAMES: &[&str] = &[
    "parseLiteral",
    "display",
    "editRepr",
    "binaryOp",
    "compare",
    "asNumber",
    "parseWith",
];

#[derive(Clone, Copy)]
enum Method {
    ParseLiteral = 0,
    Display = 1,
    EditRepr = 2,
    BinaryOp = 3,
    Compare = 4,
    AsNumber = 5,
    ParseWith = 6,
}

/// Wrap a JS type-handler object as a Rust `CustomTypeHandler`.
pub(crate) struct JsHandler {
    tag: String,
    // SendWrapper is safe here because wasm runs single-threaded; any
    // non-wasm-thread access panics — that's the intended contract.
    obj: SendWrapper<JsValue>,
    /// Methods resolved once at construction; `None` if the handler
    /// doesn't provide that method. Avoids a `Reflect::get` + `dyn_into`
    /// on every dispatch call.
    methods: SendWrapper<[Option<js_sys::Function>; 7]>,
}

impl JsHandler {
    pub(crate) fn new(obj: JsValue) -> Result<Self, JsError> {
        let tag = js_sys::Reflect::get(&obj, &JsValue::from_str("typeTag"))
            .map_err(|_| JsError::new("handler: missing `typeTag`"))?
            .as_string()
            .ok_or_else(|| JsError::new("handler: `typeTag` must be a string"))?;
        if tag.is_empty() {
            return Err(JsError::new("handler: `typeTag` must be non-empty"));
        }
        let resolve = |name: &str| -> Option<js_sys::Function> {
            let v = js_sys::Reflect::get(&obj, &JsValue::from_str(name)).ok()?;
            v.dyn_into::<js_sys::Function>().ok()
        };
        let methods = [
            resolve(METHOD_NAMES[0]),
            resolve(METHOD_NAMES[1]),
            resolve(METHOD_NAMES[2]),
            resolve(METHOD_NAMES[3]),
            resolve(METHOD_NAMES[4]),
            resolve(METHOD_NAMES[5]),
            resolve(METHOD_NAMES[6]),
        ];
        Ok(JsHandler {
            tag,
            obj: SendWrapper::new(obj),
            methods: SendWrapper::new(methods),
        })
    }

    fn method(&self, m: Method) -> Option<&js_sys::Function> {
        self.methods[m as usize].as_ref()
    }
}

impl CustomTypeHandler for JsHandler {
    fn type_tag(&self) -> &str {
        &self.tag
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        let method = self.method(Method::ParseLiteral)?;
        let result = method.call1(&self.obj, &JsValue::from_str(raw)).ok()?;
        if result.is_null() || result.is_undefined() {
            return None;
        }
        let data = js_sys::Reflect::get(&result, &JsValue::from_str("data"))
            .ok()?
            .as_string()?;
        Some(CustomValue {
            type_tag: self.tag.clone(),
            data,
        })
    }

    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        // If the JS handler exposes `parseWith`, route there; otherwise
        // fall back to the trait default (which delegates to parse_literal).
        let Some(method) = self.method(Method::ParseWith) else {
            return self.parse_literal(raw);
        };
        let args = js_args(&[JsValue::from_str(raw), JsValue::from_str(options)]);
        let result = method.apply(&self.obj, &args).ok()?;
        if result.is_null() || result.is_undefined() {
            return None;
        }
        let data = js_sys::Reflect::get(&result, &JsValue::from_str("data"))
            .ok()?
            .as_string()?;
        Some(CustomValue {
            type_tag: self.tag.clone(),
            data,
        })
    }

    fn display(&self, v: &CustomValue) -> String {
        let Some(method) = self.method(Method::Display) else {
            return v.data.clone();
        };
        method
            .call1(&self.obj, &custom_value_to_js(v))
            .ok()
            .and_then(|r| r.as_string())
            .unwrap_or_else(|| v.data.clone())
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        let Some(method) = self.method(Method::EditRepr) else {
            return v.data.clone();
        };
        method
            .call1(&self.obj, &custom_value_to_js(v))
            .ok()
            .and_then(|r| r.as_string())
            .unwrap_or_else(|| v.data.clone())
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        let method = self.method(Method::BinaryOp)?;
        let args = js_args(&[
            JsValue::from_str(op.as_str()),
            cell_value_to_js(lhs),
            cell_value_to_js(rhs),
        ]);
        match method.apply(&self.obj, &args) {
            Ok(result) => decode_optional_result(result),
            Err(e) => Some(Err(js_throw_string(&e))),
        }
    }

    fn compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        let method = self.method(Method::Compare)?;
        let args = js_args(&[
            JsValue::from_str(op.as_str()),
            cell_value_to_js(lhs),
            cell_value_to_js(rhs),
        ]);
        let result = match method.apply(&self.obj, &args) {
            Ok(r) => r,
            Err(e) => return Some(Err(js_throw_string(&e))),
        };
        if result.is_null() || result.is_undefined() {
            return None;
        }
        if let Some(err) = extract_error(&result) {
            return Some(Err(err));
        }
        result.as_bool().map(Ok)
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        let method = self.method(Method::AsNumber)?;
        method
            .call1(&self.obj, &custom_value_to_js(v))
            .ok()?
            .as_f64()
    }
}

/// Wrap a JS function (single-arg: args array) as a `CustomFunction`.
pub(crate) struct JsFunc {
    name: String,
    func: SendWrapper<js_sys::Function>,
}

impl JsFunc {
    pub(crate) fn new(name: &str, func: js_sys::Function) -> Result<Self, JsError> {
        if name.is_empty() {
            return Err(JsError::new("register_function: name must be non-empty"));
        }
        Ok(JsFunc {
            name: name.to_string(),
            func: SendWrapper::new(func),
        })
    }
}

impl CustomFunction for JsFunc {
    fn name(&self) -> &str {
        &self.name
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let js_args = js_sys::Array::new();
        for a in args {
            js_args.push(&cell_value_to_js(a));
        }
        let result = self
            .func
            .call1(&JsValue::NULL, &js_args.into())
            .map_err(|e| js_throw_string(&e))?;
        if let Some(err) = extract_error(&result) {
            return Err(err);
        }
        Ok(js_to_cell_value(&result))
    }
}

/// Marshal a `CellValue` into a JS value per the contract above.
pub(crate) fn cell_value_to_js(v: &CellValue) -> JsValue {
    match v {
        CellValue::Empty => JsValue::NULL,
        CellValue::Number(n) => JsValue::from_f64(*n),
        CellValue::String(s) => JsValue::from_str(s),
        CellValue::Boolean(b) => JsValue::from_bool(*b),
        // Errors marshal as their sentinel string ("#DIV/0!" etc.) — same
        // shape JS sees today, callers can also use Sheet::is_error()
        // server-side for typed dispatch.
        CellValue::Error(e) => JsValue::from_str(&e.to_string()),
        CellValue::Custom(cv) => custom_value_to_js(cv),
    }
}

fn custom_value_to_js(cv: &CustomValue) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("type_tag"),
        &JsValue::from_str(&cv.type_tag),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("data"),
        &JsValue::from_str(&cv.data),
    );
    obj.into()
}

/// Inverse of `cell_value_to_js`. `undefined`/`null` → Empty; JS number
/// → Number; JS string → String; `{type_tag, data}` → Custom; anything
/// else → `String(serialized)`.
pub(crate) fn js_to_cell_value(v: &JsValue) -> CellValue {
    if v.is_null() || v.is_undefined() {
        return CellValue::Empty;
    }
    if let Some(b) = v.as_bool() {
        return CellValue::Boolean(b);
    }
    if let Some(n) = v.as_f64() {
        return CellValue::Number(n);
    }
    if let Some(s) = v.as_string() {
        return CellValue::String(s);
    }
    if let Ok(type_tag) = js_sys::Reflect::get(v, &JsValue::from_str("type_tag")) {
        if let Some(tag) = type_tag.as_string() {
            let data = js_sys::Reflect::get(v, &JsValue::from_str("data"))
                .ok()
                .and_then(|d| d.as_string())
                .unwrap_or_default();
            return CellValue::Custom(CustomValue {
                type_tag: tag,
                data,
            });
        }
    }
    CellValue::String(format!("{v:?}"))
}

/// Best-effort stringification of a JS thrown value.
fn js_throw_string(err: &JsValue) -> String {
    err.as_string()
        .or_else(|| {
            // Try `.message` on Error instances.
            js_sys::Reflect::get(err, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "custom handler threw".to_string())
}

/// If `v` is an object shaped `{error: string}`, return the error message.
fn extract_error(v: &JsValue) -> Option<String> {
    let err = js_sys::Reflect::get(v, &JsValue::from_str("error")).ok()?;
    err.as_string()
}

/// Decode the binary_op return: `null`/`undefined` → decline (None);
/// `{error: "..."}` → `Some(Err)`; anything else → `Some(Ok(CellValue))`.
fn decode_optional_result(v: JsValue) -> Option<Result<CellValue, String>> {
    if v.is_null() || v.is_undefined() {
        return None;
    }
    if let Some(err) = extract_error(&v) {
        return Some(Err(err));
    }
    Some(Ok(js_to_cell_value(&v)))
}

fn js_args(vals: &[JsValue]) -> js_sys::Array {
    let a = js_sys::Array::new();
    for v in vals {
        a.push(v);
    }
    a
}
