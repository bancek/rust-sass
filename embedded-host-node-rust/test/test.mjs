import './gate.mjs';
import { compileString, compile } from "../dist/lib/index.mjs";
import { SassNumber, SassFunction } from "../dist/lib/index.mjs";
import { writeFileSync, mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

function test(name, fn) {
  try {
    const result = fn();
    if (result instanceof Promise) {
      return result.then(
        (r) => {
          console.log(`PASS ${name}`);
          return r;
        },
        (e) => console.log(`FAIL ${name}: ${e.message}`),
      );
    }
    console.log(`PASS ${name}`);
  } catch (e) {
    console.log(`FAIL ${name}: ${e.message}`);
  }
}

// --- Basic ---
test("basic SCSS", () =>
  compileString("a {b: 1px + 2px}").css === "a {\n  b: 3px;\n}");

test("indented syntax", () =>
  compileString("a\n  b: 1px + 2px", { syntax: "indented" }).css ===
  "a {\n  b: 3px;\n}");

test("CSS passthrough", () =>
  compileString("a {b: c}", { syntax: "css" }).css === "a {\n  b: c;\n}");

test("compressed output", () =>
  compileString("a {b: 1px + 2px}", { style: "compressed" }).css ===
  "a{b:3px}");

// --- Source maps ---
test("source map", () => {
  const r = compileString("a {b: 1px + 2px}", { sourceMap: true });
  return !!r.sourceMap && r.sourceMap !== "";
});

// --- File path compile ---
test("compile file", () => {
  const dir = mkdtempSync(join(tmpdir(), "sass-test-"));
  writeFileSync(join(dir, "test.scss"), "a {b: 1px + 2px}");
  return compile(join(dir, "test.scss")).css === "a {\n  b: 3px;\n}";
});

// --- Errors ---
test("syntax error", () => {
  try {
    compileString("a {b: }");
    return false;
  } catch (e) {
    return /Expected/.test(e.message);
  }
});

test("type error", () => {
  try {
    compileString("a {b: 1px + 1em}");
    return false;
  } catch (e) {
    return /incompatible/.test(e.message);
  }
});

// --- Host importer ---
test("host importer canonicalize + import", async () => {
  const imports = new Map();
  imports.set("sass:custom", "x {y: z}");

  const result = compileString('@use "custom";', {
    importers: [
      {
        canonicalize(url) {
          if (url === "custom") return new URL("sass:custom");
          return null;
        },
        load(url) {
          return { contents: imports.get(url.toString()), syntax: "scss" };
        },
      },
    ],
  });
  return result.css === "x {\n  y: z;\n}";
});

// --- File importer ---
test("file importer", async () => {
  const dir = mkdtempSync(join(tmpdir(), "sass-test-"));
  writeFileSync(join(dir, "_other.scss"), "a {b: c}");

  const result = compileString('@use "other";', {
    importers: [
      {
        findFileUrl(url) {
          return new URL(`file://${join(dir, `_${url}.scss`)}`);
        },
      },
    ],
  });
  return result.css === "a {\n  b: c;\n}";
});

// --- Custom functions ---
test("custom functions: returns Sass value", async () => {
  const result = compileString("a {b: foo()}", {
    functions: {
      "foo()": () => new SassNumber(1, "px"),
    },
  });
  return result.css === "a {\n  b: 1px;\n}";
});

test("custom functions: args by position", async () => {
  const result = compileString("a {b: foo(1px, 2px)}", {
    functions: {
      "foo($a, $b)": (args) => {
        const a = args[0].assertNumber("a");
        const b = args[1].assertNumber("b").convertValueToMatch(a, "b", "a");
        return new SassNumber(a.value + b.value).coerceToMatch(a);
      },
    },
  });
  return result.css === "a {\n  b: 3px;\n}";
});

test("custom functions: args by name", async () => {
  const result = compileString("a {b: foo($b: 2px, $a: 1px)}", {
    functions: {
      "foo($a, $b)": (args) => {
        const a = args[0].assertNumber("a");
        const b = args[1].assertNumber("b").convertValueToMatch(a, "b", "a");
        return new SassNumber(a.value + b.value).coerceToMatch(a);
      },
    },
  });
  return result.css === "a {\n  b: 3px;\n}";
});

// --- Logging ---
test("@debug", async () => {
  let logged = false;
  compileString("a {@debug hello}", {
    logger: {
      debug: () => {
        logged = true;
      },
    },
  });
  return logged;
});

test("@warn", async () => {
  let logged = false;
  compileString("a {@warn hello}", {
    logger: {
      warn: () => {
        logged = true;
      },
    },
  });
  return logged;
});

// --- First-class functions via sass:meta ---
test("first-class functions", async () => {
  const result = compileString(
    '@use "sass:meta"; a {b: meta.call(host-fn(), true)}',
    {
      functions: {
        "host-fn()": () => new SassFunction("bar($arg)", (args) => args[0]),
        "bar($arg)": (args) => args[0],
      },
    },
  );
  return result.css === "a {\n  b: true;\n}";
});

// --- Many sequential compilations ---
test("100 sequential compilations", async () => {
  for (let i = 0; i < 100; i++) {
    const r = compileString("a {b: " + i + "px}");
    if (r.css !== "a {\n  b: " + i + "px;\n}") return false;
  }
  return true;
});

// --- Concurrent async compilations ---
test("concurrent async compilations", async () => {
  const count = 20;
  const promises = [];
  for (let i = 0; i < count; i++) {
    promises.push(
      (async () => {
        const r = compileString("a {b: " + i + "px}");
        return r.css === "a {\n  b: " + i + "px;\n}";
      })(),
    );
  }
  const results = await Promise.all(promises);
  return results.every(Boolean);
});
