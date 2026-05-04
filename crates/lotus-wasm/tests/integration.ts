import { assertEquals, assertThrows } from "https://deno.land/std@0.224.0/assert/mod.ts";

// Import the wasm-pack generated module
import init, { WasmSheet } from "../pkg/lotus_wasm.js";

Deno.test("wasm integration", async (t) => {
  await init();

  await t.step("literal values", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "42"],
      ["A2", "hello"],
    ]));
    assertEquals(sheet.get("A1"), "42");
    assertEquals(sheet.get("A2"), "hello");
    assertEquals(sheet.get("Z1"), "");
    sheet.free();
  });

  await t.step("formula evaluation", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "10"],
      ["A2", "20"],
      ["A3", "=A1+A2"],
    ]));
    assertEquals(sheet.get("A3"), "30");
    sheet.free();
  });

  await t.step("SUM range", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "1"],
      ["A2", "2"],
      ["A3", "3"],
      ["A4", "=SUM(A1:A3)"],
    ]));
    assertEquals(sheet.get("A4"), "6");
    sheet.free();
  });

  await t.step("transitive dependency + update", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "1"],
      ["A2", "=A1+1"],
      ["A3", "=A2+1"],
    ]));
    assertEquals(sheet.get("A3"), "3");

    // Update A1 → should cascade
    sheet.set_cells(JSON.stringify([["A1", "10"]]));
    assertEquals(sheet.get("A2"), "11");
    assertEquals(sheet.get("A3"), "12");
    sheet.free();
  });

  await t.step("get_all returns JSON object", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "1"],
      ["B1", "=A1*2"],
    ]));
    const all = JSON.parse(sheet.get_all());
    assertEquals(all["A1"], "1");
    assertEquals(all["B1"], "2");
    sheet.free();
  });

  await t.step("circular dependency throws", () => {
    const sheet = new WasmSheet();
    assertThrows(
      () => {
        sheet.set_cells(JSON.stringify([
          ["A1", "=B1"],
          ["B1", "=A1"],
        ]));
      },
      Error,
      "CIRCULAR",
    );
    sheet.free();
  });

  await t.step("delete cell with empty string", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([["A1", "42"]]));
    assertEquals(sheet.get("A1"), "42");
    sheet.set_cells(JSON.stringify([["A1", ""]]));
    assertEquals(sheet.get("A1"), "");
    sheet.free();
  });

  await t.step("static evaluate (no sheet context)", () => {
    assertEquals(WasmSheet.evaluate("=2+3"), "5");
    assertEquals(WasmSheet.evaluate("=2^10"), "1024");
    assertEquals(WasmSheet.evaluate("=SUM(1,2,3)"), "6");
  });

  await t.step("string functions", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "hello"],
      ["A2", "=UPPER(A1)"],
      ["A3", "=LEN(A1)"],
      ["A4", '=CONCAT(A1, " world")'],
    ]));
    assertEquals(sheet.get("A2"), "HELLO");
    assertEquals(sheet.get("A3"), "5");
    assertEquals(sheet.get("A4"), "hello world");
    sheet.free();
  });

  await t.step("column range SUM(A:A)", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "1"],
      ["A2", "2"],
      ["A3", "3"],
      ["B1", "=SUM(A:A)"],
    ]));
    assertEquals(sheet.get("B1"), "6");
    sheet.free();
  });

  await t.step("IF function", () => {
    const sheet = new WasmSheet();
    sheet.set_cells(JSON.stringify([
      ["A1", "1"],
      ["A2", '=IF(A1, "yes", "no")'],
    ]));
    assertEquals(sheet.get("A2"), "yes");

    sheet.set_cells(JSON.stringify([["A1", "0"]]));
    assertEquals(sheet.get("A2"), "no");
    sheet.free();
  });

  await t.step("parse_range bounded", () => {
    const r = JSON.parse(WasmSheet.parse_range("A1:F10"));
    assertEquals(r.start, { row: 0, col: 0 });
    assertEquals(r.end_col, 5);
    assertEquals(r.end_row, 9);
    assertEquals(r.unbounded, false);
    assertEquals(r.normalized, "A1:F10");
  });

  await t.step("parse_range unbounded shapes", () => {
    const shapes: Array<[string, number, number, number, number | null, boolean]> = [
      ["A:F", 0, 0, 5, null, true],
      ["A1:F", 0, 0, 5, null, true],
      ["B5:D", 4, 1, 3, null, true],
    ];
    for (const [input, sr, sc, ec, er, unbounded] of shapes) {
      const r = JSON.parse(WasmSheet.parse_range(input));
      assertEquals(r.start, { row: sr, col: sc }, input);
      assertEquals(r.end_col, ec, input);
      assertEquals(r.end_row, er, input);
      assertEquals(r.unbounded, unbounded, input);
    }
  });

  await t.step("parse_range case-insensitive", () => {
    assertEquals(
      JSON.parse(WasmSheet.parse_range("a1:f")),
      JSON.parse(WasmSheet.parse_range("A1:F")),
    );
  });

  await t.step("parse_range reversed normalized", () => {
    const r = JSON.parse(WasmSheet.parse_range("F10:A1"));
    assertEquals(r.start, { row: 0, col: 0 });
    assertEquals(r.end_col, 5);
    assertEquals(r.end_row, 9);
  });

  await t.step("parse_range invalid throws", () => {
    for (const bad of ["", "A1", "A1:", ":F10", "A1:F10:Z20", "1:10"]) {
      assertThrows(() => WasmSheet.parse_range(bad), Error);
    }
  });

  await t.step("is_unbounded_range", () => {
    assertEquals(WasmSheet.is_unbounded_range("A1:F"), true);
    assertEquals(WasmSheet.is_unbounded_range("A:F"), true);
    assertEquals(WasmSheet.is_unbounded_range("A1:F10"), false);
    assertEquals(WasmSheet.is_unbounded_range("garbage"), false);
    assertEquals(WasmSheet.is_unbounded_range(""), false);
  });

  await t.step("parse_cell_id", () => {
    assertEquals(JSON.parse(WasmSheet.parse_cell_id("A1")), { row: 0, col: 0 });
    assertEquals(JSON.parse(WasmSheet.parse_cell_id("AB42")), { row: 41, col: 27 });
    assertThrows(() => WasmSheet.parse_cell_id("A1:B2"), Error);
    assertThrows(() => WasmSheet.parse_cell_id(""), Error);
  });

  // ── custom type handler + custom function via JS trampoline ────────

  await t.step("custom handler: parseLiteral + display", () => {
    const sheet = new WasmSheet();
    sheet.register_type({
      typeTag: "upper",
      parseLiteral(raw: string) {
        return raw.startsWith("!u:") ? { data: raw.slice(3).toUpperCase() } : null;
      },
      display(v: { type_tag: string; data: string }) {
        return `⟨${v.data}⟩`;
      },
    });
    sheet.set_cells(JSON.stringify([
      ["A1", "!u:hello"],
      ["A2", "regular"],
      ["A3", `="pre-" & A1`],
    ]));
    // A1 comes back via get_typed as {type_tag, data}.
    const a1 = sheet.get_typed("A1");
    assertEquals(a1, { type_tag: "upper", data: "HELLO" });
    assertEquals(sheet.get_typed("A2"), "regular");
    // CONCAT through `&` uses registry.display() → wrapped in ⟨…⟩.
    assertEquals(sheet.get_typed("A3"), "pre-⟨HELLO⟩");
    sheet.free();
  });

  await t.step("custom handler: binaryOp intercepts +", () => {
    const sheet = new WasmSheet();
    sheet.register_type({
      typeTag: "upper",
      parseLiteral(raw: string) {
        return raw.startsWith("!u:") ? { data: raw.slice(3).toUpperCase() } : null;
      },
      binaryOp(op: string, lhs: unknown, rhs: unknown) {
        if (op !== "+") return null;
        const unwrap = (v: unknown) =>
          typeof v === "object" && v && "data" in v ? (v as { data: string }).data : null;
        const l = unwrap(lhs);
        const r = unwrap(rhs);
        if (l === null || r === null) return null;
        return { type_tag: "upper", data: l + r };
      },
    });
    sheet.set_cells(JSON.stringify([
      ["A1", "!u:foo"],
      ["A2", "!u:bar"],
      ["A3", "=A1+A2"],
    ]));
    assertEquals(sheet.get_typed("A3"), { type_tag: "upper", data: "FOOBAR" });
    sheet.free();
  });

  await t.step("custom handler: compare intercepts =", () => {
    const sheet = new WasmSheet();
    sheet.register_type({
      typeTag: "upper",
      parseLiteral(raw: string) {
        return raw.startsWith("!u:") ? { data: raw.slice(3).toUpperCase() } : null;
      },
      compare(_op: string, lhs: unknown, rhs: unknown) {
        const unwrap = (v: unknown) =>
          typeof v === "object" && v && "data" in v ? (v as { data: string }).data : null;
        const l = unwrap(lhs);
        const r = unwrap(rhs);
        if (l === null || r === null) return null;
        return l === r;
      },
    });
    sheet.set_cells(JSON.stringify([
      ["A1", "!u:same"],
      ["A2", "!u:SAME"],        // normalized by parseLiteral → "SAME"
      ["A3", "!u:different"],
      ["B1", "=A1=A2"],
      ["B2", "=A1=A3"],
    ]));
    assertEquals(sheet.get_typed("B1"), 1);
    assertEquals(sheet.get_typed("B2"), 0);
    sheet.free();
  });

  await t.step("custom function: register AREA", () => {
    const sheet = new WasmSheet();
    sheet.register_function("AREA", (args: unknown[]) => {
      // Treat arg as JSON polygon: {points:[[x,y],...]}; trivial shoelace.
      const v = args[0];
      const data =
        typeof v === "object" && v && "data" in v
          ? (v as { data: string }).data
          : String(v);
      const pts = (JSON.parse(data) as { points: [number, number][] }).points;
      let a = 0;
      for (let i = 0; i < pts.length; i++) {
        const [x1, y1] = pts[i];
        const [x2, y2] = pts[(i + 1) % pts.length];
        a += x1 * y2 - x2 * y1;
      }
      return Math.abs(a) / 2;
    });
    sheet.register_type({
      typeTag: "polygon",
      parseLiteral(raw: string) {
        if (!raw.startsWith("POLY:")) return null;
        return { data: raw.slice(5) };
      },
    });
    sheet.set_cells(JSON.stringify([
      ["A1", 'POLY:{"points":[[0,0],[4,0],[4,3],[0,3]]}'], // 4×3 rect → area 12
      ["A2", "=AREA(A1)"],
    ]));
    assertEquals(sheet.get_typed("A2"), 12);
    sheet.free();
  });

  await t.step("custom function: error object surfaces", () => {
    const sheet = new WasmSheet();
    sheet.register_function("BOOM", () => ({ error: "#BOOM!" }));
    sheet.set_cells(JSON.stringify([["A1", "=BOOM()"]]));
    // Errors come through as strings in the cell.
    assertEquals(sheet.get_typed("A1"), "#BOOM!");
    sheet.free();
  });

  await t.step("custom handler: thrown error surfaces as cell error", () => {
    const sheet = new WasmSheet();
    sheet.register_type({
      typeTag: "u",
      parseLiteral: (raw: string) => raw === "!u" ? { data: "x" } : null,
      binaryOp: () => {
        throw new Error("#UNSUPPORTED!");
      },
    });
    sheet.set_cells(JSON.stringify([
      ["A1", "!u"],
      ["A2", "!u"],
      ["A3", "=A1+A2"],
    ]));
    const out = sheet.get_typed("A3");
    // The thrown error message surfaces as the cell value. Before the fix
    // a JS throw silently declined (fell through to numeric coercion →
    // Empty). Now it's propagated as a spreadsheet error.
    assertEquals(typeof out, "string");
    if (typeof out === "string") {
      // Error object message is "#UNSUPPORTED!"; the trampoline reads
      // `.message` off thrown Errors.
      assertEquals(out.includes("#UNSUPPORTED!"), true, `got: ${out}`);
    }
    sheet.free();
  });

  await t.step("register collisions rejected", () => {
    const sheet = new WasmSheet();
    sheet.register_type({ typeTag: "x" });
    assertThrows(() => sheet.register_type({ typeTag: "x" }));
    assertThrows(() => sheet.register_function("SUM", () => 0));
    sheet.free();
  });
});
