use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::custom::{CustomFunction, CustomTypeHandler, Registry, RegistryError};
use crate::error::{FormulaError, SheetError};
use crate::eval::Evaluator;
use crate::functions::is_builtin_function;
use crate::lexer::Lexer;
use crate::names::resolve_names;
use crate::parser::Parser;
use crate::types::{
    cell_id, col_to_index, index_to_col, ArrayValue, AstNode, CellId, CellValue, TokenType,
    MAX_RANGE_CELLS,
};

/// One change passed to [`Sheet::set_cells_typed`].
///
/// `Raw(text)` matches the [`Sheet::set_cells`] behaviour: the engine
/// parses `text` and chooses a type. `Typed(value)` skips parsing
/// entirely — the embedder has already classified the cell, so the
/// stored [`CellValue`] is used as-is. The typed entry sticks across
/// recalcs (so `Typed(String("2/4"))` stays a string instead of being
/// reclassified as `0.5`); a later `Raw` write to the same cell clears
/// it, opting back into auto-classification. Empty `Raw` deletes the
/// cell, including any typed override.
#[derive(Debug, Clone)]
pub enum CellInput {
    Raw(String),
    Typed(CellValue),
}

impl From<String> for CellInput {
    fn from(s: String) -> Self {
        CellInput::Raw(s)
    }
}

impl From<&str> for CellInput {
    fn from(s: &str) -> Self {
        CellInput::Raw(s.to_string())
    }
}

impl From<CellValue> for CellInput {
    fn from(v: CellValue) -> Self {
        CellInput::Typed(v)
    }
}

/// A rectangular spill region anchored at `anchor`. The rectangle covers
/// `rows × cols` cells starting from the anchor and extending down-and-right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillRegion {
    pub anchor: CellId,
    pub rows: u32,
    pub cols: u32,
}

/// Errors returned by spill-injection APIs like [`Sheet::pin_value`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpillError {
    /// Input wasn't a non-empty rectangular 2-D array.
    InvalidShape(String),
}

impl std::fmt::Display for SpillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpillError::InvalidShape(msg) => write!(f, "InvalidShape: {msg}"),
        }
    }
}

impl std::error::Error for SpillError {}

/// Insertion-ordered map keyed by cell id. Used for every per-cell map on
/// `Sheet` so iteration order matches the order the host populated cells —
/// makes `get_all()`, `#CIRCULAR!` errors, and JSON output reproducible.
pub type CellMap<V> = IndexMap<CellId, V>;

/// Insertion-ordered map keyed by named-range / function name.
pub type NameMap<V> = IndexMap<String, V>;

/// The three maps that always move together when an anchor's spill is
/// placed, cleared, or queried. Pulling them off `Sheet` keeps the
/// recalc loop's intent visible — `self.spill.begin_pass()` reads more
/// directly than three separate `clear()` / `take()` calls.
#[derive(Debug, Clone, Default)]
struct SpillState {
    /// Anchor → its rectangular region. Convergence between passes is
    /// determined by comparing this map.
    regions: CellMap<SpillRegion>,
    /// Reverse index: cell id → owning anchor (anchor included).
    owned_by: CellMap<CellId>,
    /// Anchor → full ArrayValue (so `A1#` reads don't re-evaluate).
    arrays: CellMap<ArrayValue>,
}

impl SpillState {
    fn region_at(&self, cell_id: &str) -> Option<&SpillRegion> {
        self.regions.get(cell_id)
    }

    fn owner_of(&self, cell_id: &str) -> Option<&CellId> {
        self.owned_by.get(cell_id)
    }

    fn array_at(&self, cell_id: &str) -> Option<&ArrayValue> {
        self.arrays.get(cell_id)
    }

    fn regions(&self) -> &CellMap<SpillRegion> {
        &self.regions
    }

    fn arrays(&self) -> &CellMap<ArrayValue> {
        &self.arrays
    }

    fn owned_keys(&self) -> impl Iterator<Item = &CellId> {
        self.owned_by.keys()
    }

    /// Snapshot the regions map for cross-pass convergence detection.
    fn regions_snapshot(&self) -> CellMap<SpillRegion> {
        self.regions.clone()
    }

    /// Reset region + array state for a fresh recalc pass and return the
    /// prior `owned_by` map so the caller can decide which previously-spilled
    /// cells need their `computed` value cleared.
    fn begin_pass(&mut self) -> CellMap<CellId> {
        self.regions.clear();
        self.arrays.clear();
        std::mem::take(&mut self.owned_by)
    }

    fn place_anchor(&mut self, anchor: CellId, region: SpillRegion, array: ArrayValue) {
        self.regions.insert(anchor.clone(), region);
        self.arrays.insert(anchor, array);
    }

    fn record_ownership(&mut self, cell: CellId, anchor: CellId) {
        self.owned_by.insert(cell, anchor);
    }
}

/// A sheet is a collection of cells, each with a raw input and a computed value.
/// The DAG tracks formula dependencies and recalculates in topological order.
#[derive(Debug, Clone)]
pub struct Sheet {
    /// Raw input strings keyed by cell ID (e.g. "A1" → "=B1+1").
    raw: CellMap<String>,
    /// Computed values keyed by cell ID.
    computed: CellMap<CellValue>,
    /// Embedder-supplied typed cell values that bypass the parse pipeline.
    /// Populated by [`Sheet::set_cells_typed`] with `CellInput::Typed`.
    /// The corresponding `raw` entry holds the rendered form (so the
    /// dep graph and bounds still see the cell), but recalc step 4
    /// short-circuits cells in this map straight into `computed` instead
    /// of re-parsing. Cleared per cell when a `Raw` write or an empty
    /// delete arrives for that cell.
    typed: CellMap<CellValue>,
    /// Workbook-global named ranges. Keys are uppercased to match the lexer.
    named_ranges: NameMap<String>,
    /// All spill-detection state — regions, ownership, anchor arrays.
    spill: SpillState,
    /// Host-injected values keyed by anchor cell. A pin overrides whatever
    /// the cell's formula would normally evaluate to — the engine skips
    /// formula eval at that cell and places the pinned array as if it had
    /// been produced by evaluation. Blocker/#SPILL! rules match native
    /// spills. Ephemeral: not persisted; the host re-installs after reload.
    pins: CellMap<ArrayValue>,
    /// Custom-type / custom-function registry, shared with each
    /// per-recalc `Evaluator` via `Arc::clone`. Defaults to an empty
    /// registry so the zero-config `Sheet::new()` path is unchanged.
    registry: Arc<Registry>,
}

/// Max recalc passes when spill topology keeps changing. Real sheets
/// converge in 2 passes; the cap just prevents runaway loops.
const MAX_RECALC_PASSES: usize = 4;

impl Sheet {
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_registry(Arc::new(Registry::default()))
    }

    /// Create a sheet whose evaluator dispatches custom literals / ops /
    /// functions through the given registry. The registry is shared
    /// between successive recalc passes; callers that need to add
    /// handlers after construction should build the registry first.
    #[must_use]
    pub fn new_with_registry(registry: Arc<Registry>) -> Self {
        Sheet {
            raw: CellMap::new(),
            computed: CellMap::new(),
            typed: CellMap::new(),
            named_ranges: NameMap::new(),
            spill: SpillState::default(),
            pins: CellMap::new(),
            registry,
        }
    }

    /// Borrow the registry this sheet was constructed with.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Register a custom-type handler on this sheet's registry. Convenience
    /// for language bindings that construct the sheet first and then want
    /// to layer on handlers. `Arc::make_mut` is cheap between recalcs
    /// (`Sheet` holds the only strong `Arc` reference then); calling
    /// during a recalc would clone, which the `&mut self` signature
    /// already prevents.
    pub fn register_type(
        &mut self,
        handler: Arc<dyn CustomTypeHandler>,
    ) -> Result<(), RegistryError> {
        Arc::make_mut(&mut self.registry).register_type(handler)
    }

    /// Register a custom function on this sheet's registry. See
    /// [`Self::register_type`] for semantics.
    pub fn register_function(
        &mut self,
        func: Arc<dyn CustomFunction>,
    ) -> Result<(), RegistryError> {
        Arc::make_mut(&mut self.registry).register_function(func)
    }

    /// Every function this sheet's evaluator can dispatch on: builtins
    /// first (in their static order), followed by runtime-registered
    /// custom functions (e.g. via [`Self::register_function`] or the
    /// `lotus-datetime` extension) in registration order. Custom
    /// functions surface a minimal [`crate::FunctionInfo`] (name only).
    pub fn list_functions(&self) -> Vec<crate::FunctionInfo> {
        crate::complete::list_functions_with_registry(&self.registry)
    }

    /// Cursor-aware signature help that resolves names through both the
    /// builtin table and this sheet's registry. Returns `None` when the
    /// cursor isn't inside any function call's parens, or when the name
    /// matches neither a builtin nor a registered custom function.
    pub fn signature_help(
        &self,
        formula: &str,
        cursor: usize,
    ) -> Option<crate::SignatureHelp> {
        crate::complete::signature_help_with_registry(&self.registry, formula, cursor)
    }

    /// Probe a specific custom-type handler with handler-defined `options`.
    /// Returns `None` if no handler is registered for `type_tag` or the
    /// handler declines. Useful for embedders that want to disambiguate
    /// input on a per-cell basis (e.g. "can `4/2` be parsed as `jdate`
    /// with strftime `%d/%m`?") and then write the result via
    /// [`Self::set_cells_typed`].
    #[must_use]
    pub fn try_parse(
        &self,
        type_tag: &str,
        raw: &str,
        options: &str,
    ) -> Option<crate::types::CustomValue> {
        self.registry.try_parse(type_tag, raw, options)
    }

    /// Set one or more cells and recalculate the sheet.
    ///
    /// Each entry is treated as [`CellInput::Raw`] — the engine parses
    /// `value` and chooses a type, exactly as before. Empty strings
    /// delete the cell. Use [`Self::set_cells_typed`] to bypass parsing
    /// and store an embedder-classified [`CellValue`] directly.
    ///
    /// # Errors
    /// Returns [`SheetError::Circular`] if a cycle is detected during the
    /// dependency-graph topo-sort.
    pub fn set_cells(
        &mut self,
        changes: &[(CellId, String)],
    ) -> Result<(), SheetError> {
        for (id, value) in changes {
            // Any prior Typed override on this cell is replaced when the
            // embedder writes Raw input — otherwise the override would
            // silently shadow the new value. Skip the hash entirely when
            // no typed entries exist anywhere (the common case): keeps
            // pure-literal recalc on the parity-with-pre-feature path.
            if !self.typed.is_empty() {
                self.typed.shift_remove(id);
            }
            if value.is_empty() {
                self.raw.shift_remove(id);
                self.computed.shift_remove(id);
            } else {
                self.raw.insert(id.clone(), value.clone());
            }
        }
        self.recalculate()
    }

    /// Set one or more cells, accepting either raw text (parsed by the
    /// engine) or pre-typed [`CellValue`]s that bypass the parse
    /// pipeline. Recalculates the sheet.
    ///
    /// `CellInput::Typed(v)` stores `v` directly into the sheet and
    /// renders it (via the registry) into the cell's raw entry so the
    /// dep graph, bounds, and editor have something to read. Recalc
    /// short-circuits typed cells — the stored value is used as-is,
    /// no re-parse — so `Typed(CellValue::String("2/4"))` stays a
    /// string across recalcs instead of being reclassified.
    ///
    /// `CellInput::Raw(text)` is identical to [`Self::set_cells`] —
    /// existing typed override on the cell is cleared, the text is
    /// re-parsed.
    ///
    /// # Errors
    /// Returns [`SheetError::Circular`] if a cycle is detected during
    /// the dependency-graph topo-sort.
    pub fn set_cells_typed(
        &mut self,
        changes: &[(CellId, CellInput)],
    ) -> Result<(), SheetError> {
        for (id, input) in changes {
            match input {
                CellInput::Raw(text) => {
                    self.typed.shift_remove(id);
                    if text.is_empty() {
                        self.raw.shift_remove(id);
                        self.computed.shift_remove(id);
                    } else {
                        self.raw.insert(id.clone(), text.clone());
                    }
                }
                CellInput::Typed(value) => {
                    let rendered = self.registry.render(value);
                    self.typed.insert(id.clone(), value.clone());
                    self.raw.insert(id.clone(), rendered);
                }
            }
        }
        self.recalculate()
    }

    /// Get the computed value of a cell.
    pub fn get(&self, id: &str) -> CellValue {
        self.computed
            .get(id)
            .cloned()
            .unwrap_or(CellValue::Empty)
    }

    /// Get all computed values as a map of cell ID to CellValue.
    /// Iteration order matches the order cells were inserted via `set_cells`.
    pub fn get_all(&self) -> &CellMap<CellValue> {
        &self.computed
    }

    /// Get the raw input of a cell.
    pub fn get_raw(&self, id: &str) -> Option<&str> {
        self.raw.get(id).map(|s| s.as_str())
    }

    /// Type tag of the computed value at `id`, when it is a custom value.
    ///
    /// Returns `None` for empty cells and for built-in variants
    /// (`Number`, `String`, `Boolean`, `Error`). Lets consumers that
    /// mix custom cell types with display logic ask "what kind is this?"
    /// without re-parsing the raw input.
    pub fn type_tag(&self, id: &str) -> Option<&str> {
        match self.computed.get(id)? {
            CellValue::Custom(cv) => Some(cv.type_tag.as_str()),
            _ => None,
        }
    }

    /// Define a workbook-global named range.
    ///
    /// `definition` is the raw text — `"=B1"`, `"=A1:A10"`, `"=SUM(A1:A10)"`,
    /// or a literal like `"0.05"`. Recalculates the sheet so dependent
    /// cells update.
    ///
    /// Names are uppercased internally (matching the lexer's identifier
    /// handling), so `set_name("TaxRate", …)` and `set_name("TAXRATE", …)`
    /// refer to the same name. Pure column-letter names like `A` or `BC`
    /// are accepted but discouraged: writing `=A:C` with `A` and `C`
    /// defined as names parses as a column range, not two name references.
    ///
    /// # Errors
    /// - [`SheetError::InvalidName`] if `name` fails validation.
    /// - [`SheetError::InvalidDefinition`] if `definition` fails to parse.
    /// - [`SheetError::Circular`] if the resulting recalc detects a cycle.
    pub fn set_name(&mut self, name: &str, definition: &str) -> Result<(), SheetError> {
        let key = validate_name(name).map_err(SheetError::InvalidName)?;
        validate_definition(definition).map_err(SheetError::InvalidDefinition)?;
        self.named_ranges.insert(key, definition.to_string());
        self.recalculate()
    }

    /// Remove a named range and recalculate.
    ///
    /// # Errors
    /// Returns [`SheetError::Circular`] if the recalc following removal
    /// detects a cycle.
    pub fn remove_name(&mut self, name: &str) -> Result<(), SheetError> {
        let key = name.to_uppercase();
        self.named_ranges.shift_remove(&key);
        self.recalculate()
    }

    /// Look up a name's raw definition.
    pub fn get_name(&self, name: &str) -> Option<&str> {
        self.named_ranges.get(&name.to_uppercase()).map(|s| s.as_str())
    }

    /// All defined names and their raw definitions.
    /// Iteration order matches insertion order.
    pub fn names(&self) -> &NameMap<String> {
        &self.named_ranges
    }

    /// If `cell_id` is the anchor of a spill region, return it.
    /// Use this from the frontend to render the raw formula only at the anchor.
    pub fn spill_at(&self, cell_id: &str) -> Option<&SpillRegion> {
        self.spill.region_at(cell_id)
    }

    /// If `cell_id` is a cell that was populated by a spill from another
    /// cell's formula (including the anchor itself), return the anchor ID.
    pub fn owner_of(&self, cell_id: &str) -> Option<&CellId> {
        self.spill.owner_of(cell_id)
    }

    /// Return the full ArrayValue at an anchor, or `None` if the cell is
    /// not an anchor. Scalar-result formulas have no anchor entry.
    pub fn spill_array(&self, cell_id: &str) -> Option<&ArrayValue> {
        self.spill.array_at(cell_id)
    }

    /// All anchors and their regions. Iteration order matches insertion order.
    pub fn spills(&self) -> &CellMap<SpillRegion> {
        self.spill.regions()
    }

    /// Pin a 2-D value at an anchor cell. The pin **replaces** whatever
    /// the cell's formula would normally evaluate to: if the cell has a
    /// raw formula it is skipped and the pinned `rows` are used instead.
    /// If the cell has no raw value, it's treated as having been
    /// produced by a formula that returned this value.
    ///
    /// Placement rules match native spill semantics:
    ///   - 1×1 → scalar at `cell_id`, no overflow.
    ///   - >1×1 → anchor + members; blockers flip the anchor to `#SPILL!`.
    ///
    /// Pinned anchors participate in the dep graph identically to native
    /// spill anchors: `=A1#` / `=SUM(A1#)` consumers see the pinned array
    /// and re-evaluate when the pin changes. Pins are ephemeral — they
    /// live on the `Sheet` instance, not in any serialized form.
    pub fn pin_value(
        &mut self,
        cell_id: &str,
        rows: Vec<Vec<CellValue>>,
    ) -> Result<(), SpillError> {
        if rows.is_empty() {
            return Err(SpillError::InvalidShape(
                "array must have at least one row".into(),
            ));
        }
        let cols = rows[0].len();
        if cols == 0 {
            return Err(SpillError::InvalidShape(
                "array must have at least one column".into(),
            ));
        }
        if rows.iter().any(|r| r.len() != cols) {
            return Err(SpillError::InvalidShape(
                "rows must all have the same length".into(),
            ));
        }
        let num_rows = rows.len() as u32;
        let flat: Vec<CellValue> = rows.into_iter().flatten().collect();
        let arr = ArrayValue::new(num_rows, cols as u32, flat);
        self.pins.insert(cell_id.to_string(), arr);
        // Cascade: pins don't introduce cycles, so drop any recalc error
        // (a pre-existing cycle would already surface as #CIRCULAR! in
        // affected computed values).
        let _ = self.recalculate();
        Ok(())
    }

    /// Remove a pin. On the next recalc the cell reverts to evaluating
    /// its own formula (or to empty if it has none).
    pub fn unpin_value(&mut self, cell_id: &str) {
        if self.pins.shift_remove(cell_id).is_some() {
            let _ = self.recalculate();
        }
    }

    /// All currently-pinned anchor cell IDs, sorted for stable output.
    pub fn pinned_cells(&self) -> Vec<CellId> {
        let mut ids: Vec<CellId> = self.pins.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// True if the cell's value comes from a pin rather than its own formula.
    pub fn is_pinned(&self, cell_id: &str) -> bool {
        self.pins.contains_key(cell_id)
    }

    /// Compute (max_row, max_col) from populated cell IDs.
    /// Returns at least (1, 1) so ranges always have some bound.
    fn bounds(&self) -> (u32, u32) {
        let mut max_row = 1u32;
        let mut max_col = 1u32;
        for id in self.raw.keys() {
            if let Some(pos) = id.find(|c: char| c.is_ascii_digit()) {
                let col = col_to_index(&id[..pos]);
                let row: u32 = id[pos..].parse().unwrap_or(0);
                max_row = max_row.max(row);
                max_col = max_col.max(col);
            }
        }
        // Account for the rectangles claimed by pinned anchors, so that
        // whole-column / whole-row ranges over a pin-only sheet still
        // see the pinned cells.
        for (id, arr) in &self.pins {
            if let Some((col, row)) = parse_cell_id(id) {
                max_row = max_row.max(row + arr.rows.saturating_sub(1));
                max_col = max_col.max(col + arr.cols.saturating_sub(1));
            }
        }
        (max_row, max_col)
    }

    /// Recalculate the sheet, iterating to a fixed point so formulas that
    /// depend on spilled cells see the post-spill state. Returns an error
    /// only for circular dependencies.
    fn recalculate(&mut self) -> Result<(), SheetError> {
        let mut prev_spills = self.spill.regions_snapshot();
        for _ in 0..MAX_RECALC_PASSES {
            self.recalc_pass()?;
            if *self.spill.regions() == prev_spills {
                return Ok(());
            }
            prev_spills = self.spill.regions_snapshot();
        }
        // Fell off the end — accept current state; real sheets converge in ≤2 passes.
        Ok(())
    }

    /// A single recalculation pass. Clears old spill state, then rebuilds
    /// dep graph (remapping spill-member refs to anchors), topo-sorts,
    /// evaluates, and places any new spills.
    fn recalc_pass(&mut self) -> Result<(), SheetError> {
        let (max_row, max_col) = self.bounds();

        // ─── Clear previous spill state ─────────────────────────────────
        // Remove computed values for cells that were spill-owned in the
        // prior pass but are not themselves authored. Keeping computed
        // for authored cells means a user-authored value stays put while
        // an anchor's formula is being re-evaluated around it.
        let prev_owned: Vec<CellId> = self.spill.owned_keys().cloned().collect();
        for cell in &prev_owned {
            if !self.raw.contains_key(cell) {
                self.computed.shift_remove(cell);
            }
        }
        // We'll rebuild spill state from scratch; `begin_pass` returns the
        // prior owned_by map so we can later compare ownership transitions.
        let prior_owned_by = self.spill.begin_pass();

        // ─── 1. Parse + resolve names for every formula. ─────────────────
        // Typed cells are skipped: their stored CellValue is authoritative,
        // and their `raw` entry is only the rendered form for display —
        // a leading `=` there must not be re-interpreted as a formula.
        // The `typed.is_empty()` short-circuit keeps the no-typed-cells
        // path (the vast majority of recalcs) free of an extra hash per
        // formula cell.
        let mut resolved: HashMap<CellId, Result<AstNode, String>> = HashMap::new();
        {
            let names = &self.named_ranges;
            let resolver = |n: &str| names.get(n).cloned();
            let typed_active = !self.typed.is_empty();
            for (id, raw) in &self.raw {
                if typed_active && self.typed.contains_key(id) {
                    continue;
                }
                if raw.starts_with('=') {
                    let result = Parser::parse(raw)
                        .and_then(|ast| resolve_names(ast, &resolver));
                    resolved.insert(id.clone(), result);
                }
            }
        }

        // ─── 2. Build dependency graph, remapping spill-member refs ─────
        // A ref to a cell that was spill-owned in the prior pass becomes
        // a ref to that cell's anchor, so topo sort puts the anchor
        // before its dependents. Pinned cells have no deps (host-injected
        // values are leaves of the graph) — their formula's refs, if any,
        // are ignored because the pin overrides formula eval.
        let mut deps: HashMap<CellId, Vec<CellId>> = HashMap::new();
        let mut reverse_deps: HashMap<CellId, Vec<CellId>> = HashMap::new();

        for (id, result) in &resolved {
            if self.pins.contains_key(id) {
                continue;
            }
            if let Ok(ast) = result {
                let raw_refs = collect_refs(ast, max_row, max_col);
                let mut remapped = Vec::with_capacity(raw_refs.len());
                for r in raw_refs {
                    let canonical = prior_owned_by.get(&r).cloned().unwrap_or(r);
                    remapped.push(canonical);
                }
                for r in &remapped {
                    reverse_deps.entry(r.clone()).or_default().push(id.clone());
                }
                deps.insert(id.clone(), remapped);
            }
        }

        // ─── 3. Topological sort (Kahn's algorithm) ─────────────────────
        // `all_ids` includes every raw-authored cell plus every pinned
        // anchor (pins may have no raw entry). Dependents of a pinned
        // anchor must wait for the pin to be placed, so pinned cells
        // count as "has value" when computing in-degrees.
        let mut all_ids: Vec<CellId> = self.raw.keys().cloned().collect();
        for id in self.pins.keys() {
            if !self.raw.contains_key(id) {
                all_ids.push(id.clone());
            }
        }

        let mut in_degree: HashMap<&CellId, usize> = HashMap::new();
        for id in &all_ids {
            let d = deps
                .get(id)
                .map(|v| {
                    v.iter()
                        .filter(|r| self.raw.contains_key(*r) || self.pins.contains_key(*r))
                        .count()
                })
                .unwrap_or(0);
            in_degree.insert(id, d);
        }

        let mut queue: VecDeque<CellId> = VecDeque::new();
        for id in &all_ids {
            if *in_degree.get(id).unwrap_or(&0) == 0 {
                queue.push_back(id.clone());
            }
        }

        let mut order: Vec<CellId> = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.clone());
            if let Some(dependents) = reverse_deps.get(&id) {
                for dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(dep.clone());
                        }
                    }
                }
            }
        }

        if order.len() < all_ids.len() {
            let mut cells: Vec<CellId> = all_ids
                .iter()
                .filter(|id| !order.contains(id))
                .map(|id| (*id).clone())
                .collect();
            // Sort for stable, reproducible error text — `all_ids` is built
            // from a HashSet inside the topo-sort, whose iteration order is
            // not guaranteed even though our top-level maps are now ordered.
            cells.sort();
            return Err(SheetError::Circular { cells });
        }

        // ─── 4. First pass: non-formula cells ──────────────────────────
        // Pinned cells are skipped here; step 5 writes the pin's array.
        // Typed cells short-circuit the parse pipeline entirely — the
        // embedder-supplied CellValue is authoritative. The
        // `typed.is_empty()` short-circuit keeps the no-typed-cells path
        // free of a hash per cell (matters for big literal loads).
        let typed_active = !self.typed.is_empty();
        for id in &order {
            if self.pins.contains_key(id) {
                continue;
            }
            if typed_active {
                if let Some(value) = self.typed.get(id) {
                    self.computed.insert(id.clone(), value.clone());
                    continue;
                }
            }
            let raw = match self.raw.get(id) {
                Some(r) => r.clone(),
                None => continue,
            };
            if !raw.starts_with('=') {
                let eval = Evaluator::with_bounds(|_| CellValue::Empty, max_row, max_col)
                    .with_registry(self.registry.clone());
                match eval.evaluate(&raw) {
                    Ok(v) => {
                        self.computed.insert(id.clone(), v);
                    }
                    Err(e) => {
                        self.computed.insert(id.clone(), CellValue::Error(e));
                    }
                }
            }
        }

        // ─── 5. Second pass: formula cells + pins, with spill placement ─
        for id in &order {
            // Pins override formula eval entirely. Place the pinned
            // array through the same path as a formula-produced array so
            // blocker / #SPILL! / ownership all match native semantics.
            if let Some(arr) = self.pins.get(id).cloned() {
                self.place_result(id, arr);
                continue;
            }
            let Some(result) = resolved.get(id) else { continue };
            match result {
                Err(e) => {
                    // Parser still surfaces String errors; convert at the
                    // storage boundary so the cell carries a typed error.
                    self.computed
                        .insert(id.clone(), CellValue::Error(FormulaError::from(e.clone())));
                }
                Ok(ast) => {
                    // Evaluate with the current `computed` snapshot, plus a
                    // spill resolver that borrows the anchor-array map
                    // directly — the closure's lifetime is bounded by this
                    // scope, so the next iteration is free to mutate it.
                    let arr = {
                        let computed = &self.computed;
                        let spill_arrays = self.spill.arrays();
                        let eval = Evaluator::with_bounds(
                            |cell_id: &str| {
                                computed
                                    .get(cell_id)
                                    .cloned()
                                    .unwrap_or(CellValue::Empty)
                            },
                            max_row,
                            max_col,
                        )
                        .with_spill_resolver(|anchor: &str| {
                            spill_arrays.get(anchor).cloned()
                        })
                        .with_registry(self.registry.clone());
                        eval.eval_ast_array(ast)
                    };
                    match arr {
                        Err(e) => {
                            self.computed.insert(id.clone(), CellValue::Error(e));
                        }
                        Ok(arr) => {
                            self.place_result(id, arr);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Write the evaluation result into `computed`, placing spills if the
    /// result is larger than 1×1 and the target rectangle is free of
    /// blockers. A blocker is any cell in the target rect (other than the
    /// anchor) that has a non-empty `raw` or is already owned by another
    /// anchor this pass.
    fn place_result(&mut self, anchor: &CellId, arr: ArrayValue) {
        if arr.is_scalar() {
            self.computed.insert(anchor.clone(), arr.first());
            return;
        }

        // Parse anchor coordinates.
        let Some((anchor_col, anchor_row)) = parse_cell_id(anchor) else {
            // Shouldn't happen — cell IDs come from set_cells which uses A1 form.
            self.computed.insert(anchor.clone(), arr.first());
            return;
        };

        // Check the target rectangle for blockers before committing.
        for r in 0..arr.rows {
            for c in 0..arr.cols {
                if r == 0 && c == 0 {
                    continue; // anchor cell itself is always OK
                }
                let cell = cell_id(&index_to_col(anchor_col + c), anchor_row + r);
                // User-authored blocker.
                let authored = self
                    .raw
                    .get(&cell)
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                // Another anchor got there first in this pass.
                let other_owner = self
                    .spill
                    .owner_of(&cell)
                    .is_some_and(|o| o != anchor);
                if authored || other_owner {
                    // Blocked — the anchor gets #SPILL!, nothing else placed.
                    self.computed
                        .insert(anchor.clone(), CellValue::Error(FormulaError::Spill));
                    return;
                }
            }
        }

        // Clear to commit: write each element and register ownership.
        for r in 0..arr.rows {
            for c in 0..arr.cols {
                let cell = cell_id(&index_to_col(anchor_col + c), anchor_row + r);
                self.computed.insert(cell.clone(), arr.get(r, c).clone());
                self.spill.record_ownership(cell, anchor.clone());
            }
        }
        let region = SpillRegion {
            anchor: anchor.clone(),
            rows: arr.rows,
            cols: arr.cols,
        };
        self.spill.place_anchor(anchor.clone(), region, arr);
    }
}

/// Split `"A1"` / `"BC42"` into `(col_1_based, row_1_based)`. `None` on parse failure.
fn parse_cell_id(id: &str) -> Option<(u32, u32)> {
    let split = id.find(|c: char| c.is_ascii_digit())?;
    let col_str = &id[..split];
    if col_str.is_empty() || !col_str.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let row: u32 = id[split..].parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((col_to_index(col_str), row))
}

impl Default for Sheet {
    fn default() -> Self {
        Self::new()
    }
}

/// Walk an AST and collect all cell IDs that it references (including range expansions).
/// `max_row` and `max_col` bound whole-column/row ranges.
fn collect_refs(node: &AstNode, max_row: u32, max_col: u32) -> Vec<CellId> {
    let mut refs = Vec::new();
    collect_refs_inner(node, &mut refs, max_row, max_col);
    refs
}

fn expand_rect(refs: &mut Vec<CellId>, start_col: &str, start_row: u32, end_col: &str, end_row: u32) {
    let sc = col_to_index(start_col);
    let ec = col_to_index(end_col);
    let sr = start_row.min(end_row);
    let er = start_row.max(end_row);
    let c_lo = sc.min(ec);
    let c_hi = sc.max(ec);
    let row_span = (er - sr + 1) as usize;
    let col_span = (c_hi - c_lo + 1) as usize;
    if row_span.saturating_mul(col_span) > MAX_RANGE_CELLS {
        return;
    }
    for row in sr..=er {
        for col in c_lo..=c_hi {
            refs.push(cell_id(&index_to_col(col), row));
        }
    }
}

fn collect_refs_inner(node: &AstNode, refs: &mut Vec<CellId>, max_row: u32, max_col: u32) {
    match node {
        AstNode::CellRef { col, row } => {
            refs.push(cell_id(col, *row));
        }
        AstNode::Range {
            start_col,
            start_row,
            end_col,
            end_row,
        } => {
            expand_rect(refs, start_col, *start_row, end_col, *end_row);
        }
        AstNode::ColumnRange { start_col, end_col } => {
            expand_rect(refs, start_col, 1, end_col, max_row);
        }
        AstNode::RowRange { start_row, end_row } => {
            let start_col = index_to_col(1);
            let end_col = index_to_col(max_col);
            expand_rect(refs, &start_col, *start_row, &end_col, *end_row);
        }
        AstNode::BinaryOp { left, right, .. } => {
            collect_refs_inner(left, refs, max_row, max_col);
            collect_refs_inner(right, refs, max_row, max_col);
        }
        AstNode::Compare { left, right, .. } => {
            collect_refs_inner(left, refs, max_row, max_col);
            collect_refs_inner(right, refs, max_row, max_col);
        }
        AstNode::UnaryOp { operand, .. } => {
            collect_refs_inner(operand, refs, max_row, max_col);
        }
        AstNode::FunctionCall { args, .. } => {
            for arg in args {
                collect_refs_inner(arg, refs, max_row, max_col);
            }
        }
        AstNode::ArrayLiteral { rows } => {
            for row in rows {
                for cell in row {
                    collect_refs_inner(cell, refs, max_row, max_col);
                }
            }
        }
        AstNode::Number(_) | AstNode::String(_) | AstNode::Boolean(_) => {}
        // `Name` nodes are removed by `resolve_names` before dep collection.
        // If one slips through (e.g. resolution returned Err) we have no
        // dependency to record — the cell will just surface the error.
        AstNode::Name(_) => {}
        // Spill ref depends on its anchor. The anchor's own recalc
        // rewrites the spill, so downstream deps fire naturally.
        AstNode::SpillRef { col, row } => {
            refs.push(cell_id(col, *row));
        }
    }
}

/// Validate a candidate name and return its uppercased canonical form.
fn validate_name(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("Name must not be empty".into());
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!("Invalid name {name:?}: must start with a letter or underscore"));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("Invalid name {name:?}: only letters, digits, and underscores allowed"));
    }
    let upper = name.to_uppercase();
    // Reject names that lex as a cell ref (e.g. `A1`, `BC42`) or as a
    // boolean literal (`TRUE`, `FALSE` — case-insensitive).
    if let Ok(toks) = Lexer::new(&format!("={upper}")).tokenize() {
        if let Some(t) = toks.first() {
            match t.token_type {
                TokenType::CellRef => {
                    return Err(format!("Invalid name {name:?}: looks like a cell reference"));
                }
                TokenType::Boolean => {
                    return Err(format!("Invalid name {name:?}: shadows a boolean literal"));
                }
                _ => {}
            }
        }
    }
    if is_builtin_function(&upper) {
        return Err(format!("Invalid name {name:?}: shadows a built-in function"));
    }
    Ok(upper)
}

/// Validate that a name's definition parses. We don't resolve nested names
/// here — those are validated lazily on the next recalculation.
fn validate_definition(definition: &str) -> Result<(), String> {
    if !definition.starts_with('=') {
        return Ok(()); // literal number/string definitions need no parse
    }
    Parser::parse(definition).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_cells() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "42".into()),
                ("A2".into(), "hello".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(42.0));
        assert_eq!(sheet.get("A2"), CellValue::String("hello".into()));
    }

    #[test]
    fn simple_formula() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "10".into()),
                ("A2".into(), "20".into()),
                ("A3".into(), "=A1+A2".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A3"), CellValue::Number(30.0));
    }

    #[test]
    fn transitive_dependency() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "5".into()),
                ("A2".into(), "=A1*2".into()),
                ("A3".into(), "=A2+A1".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A2"), CellValue::Number(10.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(15.0));
    }

    #[test]
    fn update_propagates() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), "=A1+1".into()),
                ("A3".into(), "=A2+1".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A3"), CellValue::Number(3.0));

        // Update A1 → should cascade
        sheet.set_cells(&[("A1".into(), "10".into())]).unwrap();
        assert_eq!(sheet.get("A2"), CellValue::Number(11.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(12.0));
    }

    #[test]
    fn circular_dependency_detected() {
        let mut sheet = Sheet::new();
        let result = sheet.set_cells(&[
            ("A1".into(), "=A2".into()),
            ("A2".into(), "=A1".into()),
        ]);
        let err = result.unwrap_err();
        assert!(matches!(err, SheetError::Circular { .. }), "got: {err:?}");
    }

    #[test]
    fn self_reference_detected() {
        let mut sheet = Sheet::new();
        let result = sheet.set_cells(&[("A1".into(), "=A1+1".into())]);
        assert!(matches!(result.unwrap_err(), SheetError::Circular { .. }));
    }

    #[test]
    fn delete_cell() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "42".into()),
                ("A2".into(), "=A1".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A2"), CellValue::Number(42.0));

        // Delete A1
        sheet.set_cells(&[("A1".into(), String::new())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Empty);
        assert_eq!(sheet.get("A2"), CellValue::Empty);
    }

    #[test]
    fn sum_range_formula() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), "2".into()),
                ("A3".into(), "3".into()),
                ("A4".into(), "=SUM(A1:A3)".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A4"), CellValue::Number(6.0));
    }

    #[test]
    fn mixed_formulas_and_literals() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "10".into()),
                ("B1".into(), "20".into()),
                ("C1".into(), "=A1+B1".into()),
                ("D1".into(), "=C1*2".into()),
                ("E1".into(), "=SUM(A1:D1)".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(30.0));
        assert_eq!(sheet.get("D1"), CellValue::Number(60.0));
        // SUM(10, 20, 30, 60) = 120
        assert_eq!(sheet.get("E1"), CellValue::Number(120.0));
    }

    #[test]
    fn empty_sheet() {
        let sheet = Sheet::new();
        assert_eq!(sheet.get("A1"), CellValue::Empty);
    }

    #[test]
    fn formula_referencing_empty_cell() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[("A1".into(), "=SUM(B1:B3)".into())])
            .unwrap();
        // SUM of all empty = 0
        assert_eq!(sheet.get("A1"), CellValue::Number(0.0));
    }

    #[test]
    fn if_formula() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), r#"=IF(A1, "yes", "no")"#.into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("A2"), CellValue::String("yes".into()));

        sheet.set_cells(&[("A1".into(), "0".into())]).unwrap();
        assert_eq!(sheet.get("A2"), CellValue::String("no".into()));
    }

    #[test]
    fn three_cell_cycle() {
        let mut sheet = Sheet::new();
        let result = sheet.set_cells(&[
            ("A1".into(), "=C1".into()),
            ("B1".into(), "=A1".into()),
            ("C1".into(), "=B1".into()),
        ]);
        assert!(matches!(result.unwrap_err(), SheetError::Circular { .. }));
    }

    #[test]
    fn sum_column_range() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), "2".into()),
                ("A3".into(), "3".into()),
                ("B1".into(), "=SUM(A:A)".into()),
            ])
            .unwrap();
        // SUM(A:A) should sum all populated A cells: 1+2+3=6
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
    }

    #[test]
    fn sum_multi_column_range() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("B1".into(), "2".into()),
                ("A2".into(), "3".into()),
                ("B2".into(), "4".into()),
                ("C1".into(), "=SUM(A:B)".into()),
            ])
            .unwrap();
        // SUM(A:B) across 2 rows: 1+2+3+4=10
        assert_eq!(sheet.get("C1"), CellValue::Number(10.0));
    }

    #[test]
    fn sum_row_range() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "10".into()),
                ("B1".into(), "20".into()),
                ("C1".into(), "30".into()),
                ("A2".into(), "=SUM(1:1)".into()),
            ])
            .unwrap();
        // SUM(1:1) should sum all populated row-1 cells: 10+20+30=60
        assert_eq!(sheet.get("A2"), CellValue::Number(60.0));
    }

    #[test]
    fn oversized_range_does_not_panic() {
        // An oversized range should evaluate to #SIZE! rather than
        // registering millions of dependencies or freezing.
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[("A1".into(), "=SUM(A2:A200000)".into())])
            .unwrap();
        match sheet.get("A1") {
            CellValue::Error(FormulaError::Size(_)) => {}
            v => panic!("expected #SIZE! error, got {v:?}"),
        }
    }

    #[test]
    fn small_range_still_records_deps() {
        // Dependency propagation still works for ranges under the cap.
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), "2".into()),
                ("B1".into(), "=SUM(A1:A2)".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(3.0));
        sheet.set_cells(&[("A1".into(), "10".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(12.0));
    }

    #[test]
    fn average_column_range() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "10".into()),
                ("A2".into(), "20".into()),
                ("A3".into(), "30".into()),
                ("B1".into(), "=AVERAGE(A:A)".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(20.0));
    }

    // ── named ranges ────────────────────────────────────────────────────

    #[test]
    fn named_constant_in_formula() {
        let mut sheet = Sheet::new();
        sheet.set_name("TaxRate", "0.05").unwrap();
        sheet
            .set_cells(&[
                ("A1".into(), "100".into()),
                ("B1".into(), "=A1*TaxRate".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(5.0));
    }

    #[test]
    fn named_range_in_sum() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "1".into()),
                ("A2".into(), "2".into()),
                ("A3".into(), "3".into()),
            ])
            .unwrap();
        sheet.set_name("Revenue", "=A1:A3").unwrap();
        sheet.set_cells(&[("B1".into(), "=SUM(Revenue)".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
    }

    #[test]
    fn changing_definition_propagates() {
        let mut sheet = Sheet::new();
        sheet.set_name("Rate", "0.05").unwrap();
        sheet
            .set_cells(&[
                ("A1".into(), "100".into()),
                ("B1".into(), "=A1*Rate".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(5.0));

        sheet.set_name("Rate", "0.10").unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(10.0));
    }

    #[test]
    fn definition_with_cell_ref_creates_dependency() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[("A1".into(), "10".into()), ("B1".into(), "=Foo + 1".into())])
            .unwrap();
        sheet.set_name("Foo", "=A1").unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(11.0));

        // Updating A1 must propagate through Foo to B1.
        sheet.set_cells(&[("A1".into(), "99".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(100.0));
    }

    #[test]
    fn removing_name_yields_name_error() {
        let mut sheet = Sheet::new();
        sheet.set_name("Rate", "0.05").unwrap();
        sheet
            .set_cells(&[("A1".into(), "100".into()), ("B1".into(), "=A1*Rate".into())])
            .unwrap();
        sheet.remove_name("Rate").unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Error(FormulaError::Name));
    }

    #[test]
    fn circular_name_definitions_yield_circular_error() {
        let mut sheet = Sheet::new();
        sheet.set_name("A", "=B").unwrap();
        sheet.set_name("B", "=A").unwrap();
        sheet.set_cells(&[("Z1".into(), "=A".into())]).unwrap();
        assert_eq!(sheet.get("Z1"), CellValue::Error(FormulaError::Circular));
    }

    #[test]
    fn unknown_name_yields_name_error() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=Missing".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Error(FormulaError::Name));
    }

    #[test]
    fn set_name_rejects_cell_ref_pattern() {
        let mut sheet = Sheet::new();
        assert!(sheet.set_name("A1", "0").is_err());
        assert!(sheet.set_name("BC42", "0").is_err());
    }

    #[test]
    fn set_name_rejects_builtin_function() {
        let mut sheet = Sheet::new();
        assert!(sheet.set_name("SUM", "0").is_err());
        assert!(sheet.set_name("if", "0").is_err()); // case-insensitive
    }

    #[test]
    fn set_name_rejects_boolean_literal() {
        let mut sheet = Sheet::new();
        assert!(sheet.set_name("TRUE", "0").is_err());
        assert!(sheet.set_name("FALSE", "0").is_err());
        assert!(sheet.set_name("true", "0").is_err()); // case-insensitive
        assert!(sheet.set_name("False", "0").is_err());
    }

    #[test]
    fn set_name_rejects_malformed() {
        let mut sheet = Sheet::new();
        assert!(sheet.set_name("", "0").is_err());
        assert!(sheet.set_name("1Foo", "0").is_err());
        assert!(sheet.set_name("Foo Bar", "0").is_err());
    }

    #[test]
    fn set_name_rejects_unparseable_definition() {
        let mut sheet = Sheet::new();
        assert!(sheet.set_name("Foo", "=1 + +").is_err());
    }

    #[test]
    fn concat_operator_through_dag() {
        // Regression: DAG picks up cell deps inside &-expressions.
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[
                ("A1".into(), "hello".into()),
                ("A2".into(), "world".into()),
                ("B1".into(), "=A1 & \" \" & A2".into()),
            ])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::String("hello world".into()));

        // Updating a dependency cascades.
        sheet.set_cells(&[("A2".into(), "there".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::String("hello there".into()));
    }

    // ── spill mechanics ────────────────────────────────────────────────

    #[test]
    fn spill_from_sequence() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(1.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(2.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(3.0));
        assert_eq!(sheet.get("A4"), CellValue::Number(4.0));
        assert_eq!(sheet.get("A5"), CellValue::Number(5.0));
        assert_eq!(sheet.get("A6"), CellValue::Empty);

        let region = sheet.spill_at("A1").unwrap();
        assert_eq!((region.rows, region.cols), (5, 1));
        assert_eq!(sheet.owner_of("A3"), Some(&"A1".to_string()));
        assert_eq!(sheet.owner_of("A6"), None);
    }

    #[test]
    fn spill_2d_sequence() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(2, 3)".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(1.0));
        assert_eq!(sheet.get("B1"), CellValue::Number(2.0));
        assert_eq!(sheet.get("C1"), CellValue::Number(3.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("C2"), CellValue::Number(6.0));
        assert_eq!((sheet.spill_at("A1").unwrap().rows, sheet.spill_at("A1").unwrap().cols), (2, 3));
    }

    #[test]
    fn spill_blocked_by_existing_cell() {
        let mut sheet = Sheet::new();
        // Pre-existing cell at A3 blocks the spill.
        sheet.set_cells(&[("A3".into(), "99".into())]).unwrap();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Error(FormulaError::Spill));
        // A3 keeps its user-authored value.
        assert_eq!(sheet.get("A3"), CellValue::Number(99.0));
        // A2 was not filled in (no partial spill).
        assert_eq!(sheet.get("A2"), CellValue::Empty);
        assert!(sheet.spill_at("A1").is_none());
    }

    #[test]
    fn spill_not_blocked_by_deleted_cell() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A3".into(), "99".into())]).unwrap();
        sheet.set_cells(&[("A3".into(), "".into())]).unwrap(); // delete
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert_eq!(sheet.get("A3"), CellValue::Number(3.0));
        assert!(sheet.spill_at("A1").is_some());
    }

    #[test]
    fn spill_shrinks_on_update() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert_eq!(sheet.get("A5"), CellValue::Number(5.0));

        // Shrink to 3 elements — A4 and A5 should be freed.
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(3)".into())]).unwrap();
        assert_eq!(sheet.get("A3"), CellValue::Number(3.0));
        assert_eq!(sheet.get("A4"), CellValue::Empty);
        assert_eq!(sheet.get("A5"), CellValue::Empty);
        assert!(sheet.owner_of("A4").is_none());
        assert_eq!((sheet.spill_at("A1").unwrap().rows, sheet.spill_at("A1").unwrap().cols), (3, 1));
    }

    #[test]
    fn spill_replaced_with_scalar_clears_owned() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(3)".into())]).unwrap();
        assert_eq!(sheet.get("A2"), CellValue::Number(2.0));

        sheet.set_cells(&[("A1".into(), "42".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(42.0));
        assert_eq!(sheet.get("A2"), CellValue::Empty);
        assert_eq!(sheet.get("A3"), CellValue::Empty);
        assert!(sheet.spill_at("A1").is_none());
    }

    #[test]
    fn spill_replaced_with_empty_clears_owned() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(3)".into())]).unwrap();
        sheet.set_cells(&[("A1".into(), "".into())]).unwrap(); // delete
        assert_eq!(sheet.get("A1"), CellValue::Empty);
        assert_eq!(sheet.get("A2"), CellValue::Empty);
        assert_eq!(sheet.get("A3"), CellValue::Empty);
        assert!(sheet.spill_at("A1").is_none());
    }

    #[test]
    fn broadcast_spills_row_direction() {
        // =A1:A3 * 2 in B1 should spill B1..B3.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "2".into()),
            ("A3".into(), "3".into()),
            ("B1".into(), "=A1:A3 * 2".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(2.0));
        assert_eq!(sheet.get("B2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("B3"), CellValue::Number(6.0));
    }

    #[test]
    fn two_overlapping_spills_second_errors() {
        let mut sheet = Sheet::new();
        // A1 spills 5 rows; B1 *would* also spill — but into B1..B3 which is empty.
        // Let's create a real overlap: A1 spills 3 wide into A1..C1; B1 also wants to spill wide.
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(1, 3)".into()), // A1..C1
            ("B1".into(), "=SEQUENCE(1, 2)".into()), // wants B1..C1, but B1 is owned by A1
        ]).unwrap();

        // A1 should win (topo order places authored cells in some order; we assert one of them
        // succeeded and the other is #SPILL!, rather than both).
        let a1 = sheet.get("A1");
        let b1 = sheet.get("B1");
        // Exactly one of them is an error.
        let a1_err = matches!(a1, CellValue::Error(FormulaError::Spill));
        let b1_err = matches!(b1, CellValue::Error(FormulaError::Spill));
        assert!(a1_err ^ b1_err, "expected exactly one #SPILL!, got A1={a1:?} B1={b1:?}");
    }

    #[test]
    fn cell_reading_spilled_value_works_via_fixed_point() {
        // C1 reads B3, which is a spill-member of B1's =A1:A3*2.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "2".into()),
            ("A3".into(), "3".into()),
            ("B1".into(), "=A1:A3 * 10".into()),
            ("C1".into(), "=B3 + 1".into()), // reads spill-owned B3
        ]).unwrap();
        assert_eq!(sheet.get("B3"), CellValue::Number(30.0));
        assert_eq!(sheet.get("C1"), CellValue::Number(31.0));
    }

    #[test]
    fn spill_array_exposes_full_shape() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(2, 3)".into())]).unwrap();
        let arr = sheet.spill_array("A1").unwrap();
        assert_eq!(arr.shape(), (2, 3));
        assert_eq!(*arr.get(0, 0), CellValue::Number(1.0));
        assert_eq!(*arr.get(1, 2), CellValue::Number(6.0));
    }

    #[test]
    fn empty_cell_in_spill_target_is_not_a_blocker() {
        // Previously "empty" cells (those cleared via "") have no `raw`
        // entry, so they don't block a spill.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A3".into(), "".into())]).unwrap();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert!(sheet.spill_at("A1").is_some());
    }

    #[test]
    fn filter_no_matches_yields_na_scalar() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "2".into()),
            ("B1".into(), "0".into()),
            ("B2".into(), "0".into()),
            ("C1".into(), "=FILTER(A1:A2, B1:B2)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Error(FormulaError::NotAvailable));
    }

    #[test]
    fn sum_of_spill_via_range() {
        // SUM(A1:A5) should pick up the full spilled sequence.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(5)".into()),
            ("B1".into(), "=SUM(A1:A5)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(15.0));
    }

    #[test]
    fn array_literal_spills() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "={10, 20; 30, 40}".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(10.0));
        assert_eq!(sheet.get("B1"), CellValue::Number(20.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(30.0));
        assert_eq!(sheet.get("B2"), CellValue::Number(40.0));
    }

    // ── comparisons ────────────────────────────────────────────────────

    #[test]
    fn filter_with_comparison_spills() {
        // The motivating case: =FILTER(A1#, A1#>2) — the "natural" form.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(5)".into()),
            ("C1".into(), "=FILTER(A1#, A1# > 2)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(3.0));
        assert_eq!(sheet.get("C2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("C3"), CellValue::Number(5.0));
    }

    #[test]
    fn if_with_comparison_condition_spills() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "-2".into()),
            ("A3".into(), "3".into()),
            ("B1".into(), r#"=IF(A1:A3 > 0, "pos", "neg")"#.into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::String("pos".into()));
        assert_eq!(sheet.get("B2"), CellValue::String("neg".into()));
        assert_eq!(sheet.get("B3"), CellValue::String("pos".into()));
    }

    #[test]
    fn comparison_result_is_usable_in_arithmetic() {
        // COUNTIF pattern: =SUM(A1:A5 > 2) counts values > 2.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "3".into()),
            ("A3".into(), "5".into()),
            ("A4".into(), "2".into()),
            ("A5".into(), "4".into()),
            ("B1".into(), "=SUM(A1:A5 > 2)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(3.0));
    }

    // ── spill operator A1# ─────────────────────────────────────────────

    #[test]
    fn spill_operator_reads_full_array() {
        // B1 = SUM(A1#), where A1 = SEQUENCE(5)
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(5)".into()),
            ("B1".into(), "=SUM(A1#)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(15.0));
    }

    #[test]
    fn spill_operator_multiplies_array() {
        // B1 = A1# * 2, where A1 = SEQUENCE(3). B1 should spill too.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(3)".into()),
            ("B1".into(), "=A1# * 2".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(2.0));
        assert_eq!(sheet.get("B2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("B3"), CellValue::Number(6.0));
    }

    #[test]
    fn spill_operator_on_non_anchor_yields_ref_error() {
        // A1 is just a scalar, so A1# is #REF!
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "42".into()),
            ("B1".into(), "=A1#".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Error(FormulaError::Ref));
    }

    #[test]
    fn spill_operator_updates_when_anchor_reshapes() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(3)".into()),
            ("C1".into(), "=SUM(A1#)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(6.0));
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(15.0));
    }

    #[test]
    fn spill_operator_dependencies_trigger_recalc() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(3, 1, 1, 1)".into()), // 1, 2, 3
            ("B1".into(), "=SUM(A1#)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
        // Change start value; B1 must recompute.
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(3, 1, 10, 1)".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(33.0));
    }

    #[test]
    fn named_range_points_to_spill_operator() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        sheet.set_name("MyData", "=A1#").unwrap();
        sheet.set_cells(&[("B1".into(), "=SUM(MyData)".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(15.0));
    }

    #[test]
    fn named_range_spill_reference_in_filter() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A1".into(), "=SEQUENCE(5)".into())]).unwrap();
        sheet.set_name("Data", "=A1#").unwrap();
        sheet.set_cells(&[("C1".into(), "=FILTER(Data, Data > 2)".into())]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(3.0));
        assert_eq!(sheet.get("C2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("C3"), CellValue::Number(5.0));
    }

    #[test]
    fn named_range_pointing_to_range_behaves_like_spill_ref() {
        // A named range defined as a plain range (=A1:A5) should also
        // work as an array when referenced.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "10".into()),
            ("A2".into(), "20".into()),
            ("A3".into(), "30".into()),
        ]).unwrap();
        sheet.set_name("Data", "=A1:A3").unwrap();
        sheet.set_cells(&[("B1".into(), "=SUM(Data)".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(60.0));
    }

    #[test]
    fn spill_operator_extract_refs_has_spill_kind() {
        use crate::extract_refs;
        let refs = extract_refs("=SUM(A1#)");
        let spill = refs.iter().find(|r| matches!(r.kind, crate::RefKind::SpillRef)).unwrap();
        assert_eq!(spill.text, "A1#");
        assert_eq!(spill.cells.len(), 1);
        assert_eq!(crate::range::format_cell_id(spill.cells[0]), "A1");
    }

    // ── `$` absolute markers are semantic no-ops ──────────────────────

    #[test]
    fn absolute_cell_ref_evaluates_like_relative() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "10".into()),
            ("B1".into(), "2".into()),
            ("C1".into(), "=$A$1*B1".into()),
            ("C2".into(), "=$A1*B$1".into()),
            ("C3".into(), "=A$1*$B1".into()),
        ]).unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(20.0));
        assert_eq!(sheet.get("C2"), CellValue::Number(20.0));
        assert_eq!(sheet.get("C3"), CellValue::Number(20.0));
    }

    #[test]
    fn absolute_refs_participate_in_dep_tracking() {
        // $A$1 depends on A1: updating A1 should cascade.
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "10".into()),
            ("B1".into(), "=$A$1+5".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(15.0));
        sheet.set_cells(&[("A1".into(), "100".into())]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(105.0));
    }

    #[test]
    fn absolute_range_evaluates_like_relative() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "2".into()),
            ("A3".into(), "3".into()),
            ("B1".into(), "=SUM($A$1:$A$3)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
    }

    #[test]
    fn absolute_whole_col_range_evaluates() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "1".into()),
            ("A2".into(), "2".into()),
            ("A3".into(), "3".into()),
            ("B1".into(), "=SUM($A:$A)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
    }

    #[test]
    fn absolute_spill_ref_evaluates() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[
            ("A1".into(), "=SEQUENCE(3)".into()),
            ("B1".into(), "=SUM($A$1#)".into()),
        ]).unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(6.0));
    }

    // ── pin_value / host-injected spills ──────────────────────────────

    fn rows_num(vals: &[&[f64]]) -> Vec<Vec<CellValue>> {
        vals.iter()
            .map(|row| row.iter().map(|n| CellValue::Number(*n)).collect())
            .collect()
    }

    #[test]
    fn pin_3x1_on_empty_cell_spills() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[10.0], &[20.0], &[30.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(10.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(20.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(30.0));
        let region = sheet.spill_at("A1").unwrap();
        assert_eq!((region.rows, region.cols), (3, 1));
    }

    #[test]
    fn pin_2x2_on_empty_cell_spills() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0, 2.0], &[3.0, 4.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(1.0));
        assert_eq!(sheet.get("B1"), CellValue::Number(2.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(3.0));
        assert_eq!(sheet.get("B2"), CellValue::Number(4.0));
        assert_eq!(sheet.owner_of("B2"), Some(&"A1".to_string()));
    }

    #[test]
    fn pin_1x1_writes_scalar() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[42.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(42.0));
        assert!(sheet.spill_at("A1").is_none());
    }

    #[test]
    fn sum_of_pinned_spill_via_spill_ref() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0], &[4.0]]))
            .unwrap();
        sheet
            .set_cells(&[("B1".into(), "=SUM(A1#)".into())])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(10.0));
    }

    #[test]
    fn pin_overrides_formula_raw_value() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[("A1".into(), "=SEQUENCE(5)".into())])
            .unwrap();
        // Before pin: native 5-row spill.
        assert_eq!(sheet.get("A5"), CellValue::Number(5.0));
        sheet
            .pin_value("A1", rows_num(&[&[100.0], &[200.0], &[300.0]]))
            .unwrap();
        // Pin wins: only 3 rows, with pinned values.
        assert_eq!(sheet.get("A1"), CellValue::Number(100.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(200.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(300.0));
        assert_eq!(sheet.get("A4"), CellValue::Empty);
        assert_eq!(sheet.get("A5"), CellValue::Empty);
        let region = sheet.spill_at("A1").unwrap();
        assert_eq!((region.rows, region.cols), (3, 1));
    }

    #[test]
    fn pin_blocked_by_authored_cell_yields_spill_error() {
        let mut sheet = Sheet::new();
        sheet.set_cells(&[("A2".into(), "blocker".into())]).unwrap();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Error(FormulaError::Spill));
        assert_eq!(sheet.get("A2"), CellValue::String("blocker".into()));
        assert!(sheet.spill_at("A1").is_none());
    }

    #[test]
    fn repin_different_shape_clears_old_members_and_notifies_dependents() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0], &[4.0]]))
            .unwrap();
        sheet
            .set_cells(&[("C1".into(), "=SUM(A1#)".into())])
            .unwrap();
        assert_eq!(sheet.get("A4"), CellValue::Number(4.0));
        assert_eq!(sheet.get("C1"), CellValue::Number(10.0));
        // Re-pin with a smaller shape; old members A3/A4 get cleared and
        // the SUM(A1#) dependent re-evaluates.
        sheet
            .pin_value("A1", rows_num(&[&[5.0], &[6.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(5.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(6.0));
        assert_eq!(sheet.get("A3"), CellValue::Empty);
        assert_eq!(sheet.get("A4"), CellValue::Empty);
        assert_eq!(sheet.get("C1"), CellValue::Number(11.0));
    }

    #[test]
    fn unpin_reverts_to_formula_eval() {
        let mut sheet = Sheet::new();
        sheet
            .set_cells(&[("A1".into(), "=SEQUENCE(3)".into())])
            .unwrap();
        sheet
            .pin_value("A1", rows_num(&[&[99.0]]))
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(99.0));
        assert_eq!(sheet.get("A2"), CellValue::Empty);

        sheet.unpin_value("A1");
        assert_eq!(sheet.get("A1"), CellValue::Number(1.0));
        assert_eq!(sheet.get("A2"), CellValue::Number(2.0));
        assert_eq!(sheet.get("A3"), CellValue::Number(3.0));
    }

    #[test]
    fn unpin_clears_spill_when_no_formula() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0]]))
            .unwrap();
        assert!(sheet.spill_at("A1").is_some());
        sheet.unpin_value("A1");
        assert!(sheet.spill_at("A1").is_none());
        assert_eq!(sheet.owner_of("A2"), None);
        assert_eq!(sheet.get("A1"), CellValue::Empty);
    }

    #[test]
    fn pin_value_rejects_empty_outer() {
        let mut sheet = Sheet::new();
        let err = sheet.pin_value("A1", Vec::<Vec<CellValue>>::new()).unwrap_err();
        assert!(matches!(err, SpillError::InvalidShape(_)));
    }

    #[test]
    fn pin_value_rejects_empty_inner() {
        let mut sheet = Sheet::new();
        let err = sheet.pin_value("A1", vec![Vec::<CellValue>::new()]).unwrap_err();
        assert!(matches!(err, SpillError::InvalidShape(_)));
    }

    #[test]
    fn pin_value_rejects_ragged_rows() {
        let mut sheet = Sheet::new();
        let err = sheet
            .pin_value(
                "A1",
                vec![
                    vec![CellValue::Number(1.0), CellValue::Number(2.0)],
                    vec![CellValue::Number(3.0)],
                ],
            )
            .unwrap_err();
        assert!(matches!(err, SpillError::InvalidShape(_)));
    }

    #[test]
    fn filter_on_pinned_spill() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0], &[4.0], &[5.0]]))
            .unwrap();
        sheet
            .set_cells(&[("C1".into(), "=FILTER(A1#, A1# > 2)".into())])
            .unwrap();
        assert_eq!(sheet.get("C1"), CellValue::Number(3.0));
        assert_eq!(sheet.get("C2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("C3"), CellValue::Number(5.0));
    }

    #[test]
    fn broadcast_over_pinned_region() {
        let mut sheet = Sheet::new();
        sheet
            .pin_value("A1", rows_num(&[&[1.0], &[2.0], &[3.0]]))
            .unwrap();
        sheet
            .set_cells(&[("B1".into(), "=A1# * 2".into())])
            .unwrap();
        assert_eq!(sheet.get("B1"), CellValue::Number(2.0));
        assert_eq!(sheet.get("B2"), CellValue::Number(4.0));
        assert_eq!(sheet.get("B3"), CellValue::Number(6.0));
    }

    #[test]
    fn pinned_cells_and_is_pinned() {
        let mut sheet = Sheet::new();
        assert_eq!(sheet.pinned_cells(), Vec::<String>::new());
        assert!(!sheet.is_pinned("A1"));
        sheet
            .pin_value("A1", rows_num(&[&[1.0]]))
            .unwrap();
        sheet
            .pin_value("C5", rows_num(&[&[9.0, 8.0]]))
            .unwrap();
        assert_eq!(sheet.pinned_cells(), vec!["A1".to_string(), "C5".to_string()]);
        assert!(sheet.is_pinned("A1"));
        assert!(sheet.is_pinned("C5"));
        assert!(!sheet.is_pinned("B1"));
        sheet.unpin_value("A1");
        assert_eq!(sheet.pinned_cells(), vec!["C5".to_string()]);
    }

    #[test]
    fn pin_string_values_for_loading_state() {
        // Simulate the #LOADING! / SQL flow: the host pins a sentinel
        // string while an async fetch is in flight.
        let mut sheet = Sheet::new();
        sheet
            .pin_value(
                "A1",
                vec![vec![CellValue::String("#LOADING!".into())]],
            )
            .unwrap();
        assert_eq!(sheet.get("A1"), CellValue::String("#LOADING!".into()));
        sheet
            .pin_value(
                "A1",
                vec![
                    vec![CellValue::String("id".into()), CellValue::String("name".into())],
                    vec![CellValue::String("1".into()), CellValue::String("alex".into())],
                ],
            )
            .unwrap();
        assert_eq!(sheet.get("B2"), CellValue::String("alex".into()));
    }

    #[test]
    fn case_insensitive_name_lookup() {
        let mut sheet = Sheet::new();
        sheet.set_name("MyName", "42").unwrap();
        sheet.set_cells(&[("A1".into(), "=MYNAME".into())]).unwrap();
        assert_eq!(sheet.get("A1"), CellValue::Number(42.0));
        assert_eq!(sheet.get_name("myname"), Some("42"));
    }

    // ── instance-level list_functions / signature_help ──

    struct StubFn(&'static str);
    impl CustomFunction for StubFn {
        fn name(&self) -> &str {
            self.0
        }
        fn call(&self, _args: &[CellValue]) -> Result<CellValue, String> {
            Ok(CellValue::Empty)
        }
    }

    #[test]
    fn list_functions_includes_runtime_registrations() {
        let mut s = Sheet::new();
        let baseline = s.list_functions().len();
        s.register_function(Arc::new(StubFn("MY_FN"))).unwrap();
        let after = s.list_functions();
        assert_eq!(after.len(), baseline + 1);
        assert!(after.iter().any(|f| f.name == "MY_FN"));
        // Builtins are still present.
        assert!(after.iter().any(|f| f.name == "SUM"));
    }

    #[test]
    fn signature_help_resolves_host_registered_function() {
        let mut s = Sheet::new();
        s.register_function(Arc::new(StubFn("MY_FN"))).unwrap();
        let sig = s.signature_help("=MY_FN(", 7).unwrap();
        assert_eq!(sig.function.name, "MY_FN");
        assert_eq!(sig.active_param, 0);
    }

    #[test]
    fn signature_help_falls_back_to_builtin_lookup() {
        let s = Sheet::new();
        let sig = s.signature_help("=SUM(", 5).unwrap();
        assert_eq!(sig.function.name, "SUM");
    }

    #[test]
    fn signature_help_unknown_returns_none_on_instance() {
        let s = Sheet::new();
        assert!(s.signature_help("=NOPE(", 6).is_none());
    }

    struct TagAt(&'static str);
    impl CustomTypeHandler for TagAt {
        fn type_tag(&self) -> &str {
            self.0
        }
        fn parse_literal(&self, raw: &str) -> Option<crate::types::CustomValue> {
            raw.strip_prefix('@').map(|rest| crate::types::CustomValue {
                type_tag: self.0.into(),
                data: rest.into(),
            })
        }
    }

    #[test]
    fn type_tag_returns_none_for_empty_cell() {
        let s = Sheet::new();
        assert!(s.type_tag("A1").is_none());
    }

    #[test]
    fn type_tag_returns_none_for_builtin_variants() {
        let mut s = Sheet::new();
        s.set_cells(&[
            ("A1".into(), "42".into()),
            ("A2".into(), "hello".into()),
            ("A3".into(), "=1/0".into()),
        ])
        .unwrap();
        assert!(s.type_tag("A1").is_none());
        assert!(s.type_tag("A2").is_none());
        assert!(s.type_tag("A3").is_none());
    }

    #[test]
    fn type_tag_returns_tag_for_custom_value() {
        let mut s = Sheet::new();
        s.register_type(Arc::new(TagAt("mytag"))).unwrap();
        s.set_cells(&[("A1".into(), "@hello".into())]).unwrap();
        assert_eq!(s.type_tag("A1"), Some("mytag"));
    }
}
