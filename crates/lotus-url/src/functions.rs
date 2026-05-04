use std::sync::Arc;

use lotus_core::{CellValue, CustomFunction, FormulaError, Registry, RegistryError};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use url::Url;

pub(crate) fn register_url_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(UrlSchemeFn))?;
    registry.register_function(Arc::new(UrlHostFn))?;
    registry.register_function(Arc::new(UrlPortFn))?;
    registry.register_function(Arc::new(UrlPathFn))?;
    registry.register_function(Arc::new(UrlPathSegmentFn))?;
    registry.register_function(Arc::new(UrlQueryFn))?;
    registry.register_function(Arc::new(UrlFragmentFn))?;
    registry.register_function(Arc::new(UrlParamFn))?;
    registry.register_function(Arc::new(UrlValidFn))?;
    registry.register_function(Arc::new(UrlEncodeFn))?;
    registry.register_function(Arc::new(UrlDecodeFn))?;
    registry.register_function(Arc::new(UrlJoinFn))?;
    Ok(())
}

/// Component-style percent-encoding set: encode everything that isn't
/// `[A-Za-z0-9]`. Matches the conservative default used by most
/// `encodeURIComponent`-style helpers — over-encodes a few unreserved
/// punctuation chars (`-_.~`) but never under-encodes.
const COMPONENT_SET: &AsciiSet = NON_ALPHANUMERIC;

fn arg_text<'a>(args: &'a [CellValue], position: usize, fn_name: &str) -> Result<&'a str, String> {
    let v = args.get(position - 1).ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string()
    })?;
    match v {
        CellValue::String(s) => Ok(s.as_str()),
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be text"
        ))
        .to_string()),
    }
}

fn arg_number(args: &[CellValue], position: usize, fn_name: &str) -> Result<f64, String> {
    let v = args.get(position - 1).ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string()
    })?;
    v.as_number().ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: arg #{position} must be a number")).to_string()
    })
}

fn parse_url(s: &str, fn_name: &str) -> Result<Url, String> {
    Url::parse(s)
        .map_err(|e| FormulaError::value(format!("{fn_name}: invalid URL ({e})")).to_string())
}

fn check_arity(fn_name: &str, args: &[CellValue], expected: usize) -> Result<(), String> {
    if args.len() != expected {
        return Err(FormulaError::value(format!(
            "{fn_name}: expected {expected} arg{}",
            if expected == 1 { "" } else { "s" }
        ))
        .to_string());
    }
    Ok(())
}

/// `URL_SCHEME(url) -> text` — e.g. `"https"`.
struct UrlSchemeFn;
impl CustomFunction for UrlSchemeFn {
    fn name(&self) -> &str {
        "URL_SCHEME"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_SCHEME", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_SCHEME")?, "URL_SCHEME")?;
        Ok(CellValue::String(url.scheme().to_string()))
    }
}

/// `URL_HOST(url) -> text` — hostname only, no port. Empty if absent
/// (e.g. `data:` URLs).
struct UrlHostFn;
impl CustomFunction for UrlHostFn {
    fn name(&self) -> &str {
        "URL_HOST"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_HOST", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_HOST")?, "URL_HOST")?;
        Ok(CellValue::String(
            url.host_str().unwrap_or("").to_string(),
        ))
    }
}

/// `URL_PORT(url) -> number` — explicit port, or `0` if the URL didn't
/// specify one (so `https://x.com` → `0`, `https://x.com:8443` → `8443`).
struct UrlPortFn;
impl CustomFunction for UrlPortFn {
    fn name(&self) -> &str {
        "URL_PORT"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_PORT", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_PORT")?, "URL_PORT")?;
        Ok(CellValue::Number(f64::from(url.port().unwrap_or(0))))
    }
}

/// `URL_PATH(url) -> text` — full path with leading `/`. Use
/// `URL_PATH_SEGMENT` for individual decoded segments.
struct UrlPathFn;
impl CustomFunction for UrlPathFn {
    fn name(&self) -> &str {
        "URL_PATH"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_PATH", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_PATH")?, "URL_PATH")?;
        Ok(CellValue::String(url.path().to_string()))
    }
}

/// `URL_PATH_SEGMENT(url, n) -> text`. 1-based index; negative `n` counts
/// from the end (`-1` is the last segment). Empty segments (leading and
/// trailing slashes) are skipped, so `https://x.com/foo/` has one
/// segment, not two. Returned text is percent-decoded.
struct UrlPathSegmentFn;
impl CustomFunction for UrlPathSegmentFn {
    fn name(&self) -> &str {
        "URL_PATH_SEGMENT"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_PATH_SEGMENT", args, 2)?;
        let url = parse_url(arg_text(args, 1, "URL_PATH_SEGMENT")?, "URL_PATH_SEGMENT")?;
        let n_raw = arg_number(args, 2, "URL_PATH_SEGMENT")?;
        if !n_raw.is_finite() || n_raw.fract() != 0.0 || n_raw == 0.0 {
            return Err(FormulaError::value(
                "URL_PATH_SEGMENT: index must be a non-zero whole number",
            )
            .to_string());
        }
        // Cannot-be-base URLs (e.g. `mailto:`) yield no segments.
        let segments: Vec<&str> = match url.path_segments() {
            Some(it) => it.filter(|s| !s.is_empty()).collect(),
            None => return Ok(CellValue::String(String::new())),
        };
        let len = segments.len() as i64;
        let n = n_raw as i64;
        let idx = if n > 0 { n - 1 } else { len + n };
        if idx < 0 || idx >= len {
            return Ok(CellValue::String(String::new()));
        }
        let raw = segments[idx as usize];
        let decoded = percent_decode_str(raw)
            .decode_utf8()
            .map_err(|_| {
                FormulaError::value("URL_PATH_SEGMENT: segment is not valid UTF-8").to_string()
            })?;
        Ok(CellValue::String(decoded.into_owned()))
    }
}

/// `URL_QUERY(url) -> text` — the part after `?`, without the leading
/// `?`. Empty text if absent.
struct UrlQueryFn;
impl CustomFunction for UrlQueryFn {
    fn name(&self) -> &str {
        "URL_QUERY"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_QUERY", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_QUERY")?, "URL_QUERY")?;
        Ok(CellValue::String(url.query().unwrap_or("").to_string()))
    }
}

/// `URL_FRAGMENT(url) -> text` — the part after `#`, without the
/// leading `#`. Empty text if absent.
struct UrlFragmentFn;
impl CustomFunction for UrlFragmentFn {
    fn name(&self) -> &str {
        "URL_FRAGMENT"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_FRAGMENT", args, 1)?;
        let url = parse_url(arg_text(args, 1, "URL_FRAGMENT")?, "URL_FRAGMENT")?;
        Ok(CellValue::String(url.fragment().unwrap_or("").to_string()))
    }
}

/// `URL_PARAM(url, key) -> text` — first value for `key` in the query
/// string. Already percent-decoded by `query_pairs`. Empty text if the
/// key is absent.
struct UrlParamFn;
impl CustomFunction for UrlParamFn {
    fn name(&self) -> &str {
        "URL_PARAM"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_PARAM", args, 2)?;
        let url = parse_url(arg_text(args, 1, "URL_PARAM")?, "URL_PARAM")?;
        let key = arg_text(args, 2, "URL_PARAM")?;
        let val = url
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        Ok(CellValue::String(val))
    }
}

/// `URL_VALID(url) -> bool` — whether `url` parses cleanly. Use this
/// before the extractors if a column may contain garbage; the
/// extractors themselves error on invalid input.
struct UrlValidFn;
impl CustomFunction for UrlValidFn {
    fn name(&self) -> &str {
        "URL_VALID"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_VALID", args, 1)?;
        let s = arg_text(args, 1, "URL_VALID")?;
        Ok(CellValue::Boolean(Url::parse(s).is_ok()))
    }
}

/// `URL_ENCODE(text) -> text` — percent-encode for use as a URL
/// component (path segment or query value). Encodes everything outside
/// `[A-Za-z0-9]`, including space (→ `%20`), `/`, `?`, `=`, `&`.
struct UrlEncodeFn;
impl CustomFunction for UrlEncodeFn {
    fn name(&self) -> &str {
        "URL_ENCODE"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_ENCODE", args, 1)?;
        let s = arg_text(args, 1, "URL_ENCODE")?;
        Ok(CellValue::String(
            utf8_percent_encode(s, COMPONENT_SET).to_string(),
        ))
    }
}

/// `URL_DECODE(text) -> text` — reverse of `URL_ENCODE`. Errors if the
/// decoded bytes aren't valid UTF-8.
struct UrlDecodeFn;
impl CustomFunction for UrlDecodeFn {
    fn name(&self) -> &str {
        "URL_DECODE"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_DECODE", args, 1)?;
        let s = arg_text(args, 1, "URL_DECODE")?;
        let decoded = percent_decode_str(s).decode_utf8().map_err(|_| {
            FormulaError::value("URL_DECODE: result is not valid UTF-8").to_string()
        })?;
        Ok(CellValue::String(decoded.into_owned()))
    }
}

/// `URL_JOIN(base, relative) -> text` — resolve `relative` against
/// `base` per RFC 3986. `base` must be an absolute URL.
struct UrlJoinFn;
impl CustomFunction for UrlJoinFn {
    fn name(&self) -> &str {
        "URL_JOIN"
    }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        check_arity("URL_JOIN", args, 2)?;
        let base = parse_url(arg_text(args, 1, "URL_JOIN")?, "URL_JOIN")?;
        let rel = arg_text(args, 2, "URL_JOIN")?;
        let joined = base
            .join(rel)
            .map_err(|e| FormulaError::value(format!("URL_JOIN: {e}")).to_string())?;
        Ok(CellValue::String(joined.into()))
    }
}
