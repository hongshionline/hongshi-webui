/*
 * Find identifiers a web script reads but never declares.
 *
 * This exists because of a real bug: `pages.js` used `sessionKv` in three places
 * without ever declaring it. Reading an undeclared name throws a ReferenceError —
 * and because the page builder was called bare from the router, the throw travelled
 * up through `boot()` and silently cancelled everything after it (the health poll,
 * the one-second tick, the drawer wiring). The only visible symptom was a status
 * pill stuck on "连接中…" and an empty section, which no test noticed.
 *
 * `node --check` cannot see this: it is a syntax check, and the file is perfectly
 * valid syntax. So the scan below does a deliberately crude job of it — strip
 * comments and string-ish literals, collect every declaration, then flag the names
 * that are read but declared nowhere.
 *
 * It is a heuristic and it errs toward silence: anything it cannot reason about is
 * ignored rather than reported, so a clean run is evidence and a noisy one is a bug
 * in this file. Known browser globals are listed explicitly; anything else a script
 * legitimately reaches for from outside itself belongs in ALLOWED too.
 *
 *   node scripts/undeclared.mjs web/app.js web/pages.js
 *
 * Exits non-zero when it finds one, so it can gate a build.
 */

import { readFileSync } from "node:fs";

/* Reserved words. The scan matches identifiers lexically, so without this list it
   reports `var`, `return` and `typeof` as undeclared names. */
const KEYWORDS = new Set([
  "var", "let", "const", "function", "class", "return", "if", "else", "for",
  "while", "do", "switch", "case", "default", "break", "continue", "try",
  "catch", "finally", "throw", "new", "delete", "typeof", "instanceof", "in",
  "of", "void", "yield", "await", "async", "static", "get", "set", "super",
  "extends", "import", "export", "null", "true", "false",
]);

/* Names that are legitimately not declared in the file that uses them: browser
   globals, plus the shared surface `app.js` publishes for `pages.js`. */
const ALLOWED = new Set([
  // language / literals
  "undefined", "NaN", "Infinity", "globalThis", "arguments", "this",
  // browser globals
  "window", "document", "navigator", "location", "console", "history",
  "localStorage", "sessionStorage", "fetch", "setTimeout", "clearTimeout",
  "setInterval", "clearInterval", "requestAnimationFrame", "cancelAnimationFrame",
  "matchMedia", "MutationObserver", "IntersectionObserver", "ResizeObserver",
  "CustomEvent", "Event", "URL", "URLSearchParams", "Blob", "FileReader",
  "AbortController", "Promise", "Object", "Array", "String", "Number", "Boolean",
  "Math", "JSON", "Date", "RegExp", "Error", "TypeError", "Symbol", "Map", "Set",
  "WeakMap", "WeakSet", "Intl", "encodeURIComponent", "decodeURIComponent",
  "encodeURI", "decodeURI", "parseInt", "parseFloat", "isNaN", "isFinite",
  "alert", "confirm", "prompt", "crypto", "performance", "structuredClone",
  // the bridge between the two classic scripts
  "Shell",
  ...KEYWORDS,
]);

const DECL = /\b(?:var|let|const|function|class)\s+([A-Za-z_$][\w$]*)/g;
/* Both named and anonymous function expressions: `function (response) {}` is how
   every callback in these files is written, and missing its parameters made the
   first version of this scan report a dozen false positives. */
const FUNC_PARAMS = /\bfunction\s*(?:[A-Za-z_$][\w$]*\s*)?\(([^)]*)\)/g;
const ARROW_PARAMS = /\(([^()]*)\)\s*=>/g;
/* `src` in `catch (err)` and the names in a destructuring pattern both count. */
const CATCH = /\bcatch\s*\(\s*([A-Za-z_$][\w$]*)/g;
const BARE_PARAM = /^\s*([A-Za-z_$][\w$]*)\s*=>/gm;

/* Replace comments and literal contents with spaces, so a name mentioned inside a
   string or a comment is never mistaken for a use. Length is preserved so reported
   positions stay meaningful. */
function strip(source) {
  let out = "";
  let i = 0;
  const n = source.length;
  const blank = (s) => s.replace(/[^\n]/g, " ");

  while (i < n) {
    const c = source[i];
    const next = source[i + 1];

    if (c === "/" && next === "/") {
      const end = source.indexOf("\n", i);
      const stop = end < 0 ? n : end;
      out += blank(source.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2);
      const stop = end < 0 ? n : end + 2;
      out += blank(source.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < n) {
        if (source[j] === "\\") { j += 2; continue; }
        if (source[j] === c) break;
        j += 1;
      }
      out += c + blank(source.slice(i + 1, Math.min(j, n))) + (j < n ? c : "");
      i = Math.min(j + 1, n);
      continue;
    }
    /* A regex literal is only a regex where a value may start; after an identifier,
       a number or a closing bracket it is division. That distinction is the whole
       heuristic — when in doubt this treats `/` as division and moves on. */
    if (c === "/" && /[=(,:[!&|?{};+\-*%<>~^]|^$/.test(out.trimEnd().slice(-1))) {
      let j = i + 1;
      let inClass = false;
      let closed = false;
      while (j < n) {
        const d = source[j];
        if (d === "\\") { j += 2; continue; }
        if (d === "\n") break;
        if (d === "[") inClass = true;
        else if (d === "]") inClass = false;
        else if (d === "/" && !inClass) { closed = true; j += 1; break; }
        j += 1;
      }
      if (closed) {
        /* Swallow the flags too, or `/&/g` leaves a stray `g` that reads as a use. */
        let k = j;
        while (k < n && /[a-z]/.test(source[k])) k += 1;
        out += blank(source.slice(i, k));
        i = k;
        continue;
      }
    }
    out += c;
    i += 1;
  }
  return out;
}

function declaredNames(code) {
  const names = new Set(ALLOWED);
  const add = (raw) => {
    for (const piece of String(raw).split(",")) {
      const name = piece.replace(/=.*$/s, "").replace(/[:{}[\].]/g, " ").trim().split(/\s+/)[0];
      if (name && /^[A-Za-z_$][\w$]*$/.test(name)) names.add(name);
    }
  };
  let m;
  for (const re of [DECL, CATCH]) {
    re.lastIndex = 0;
    while ((m = re.exec(code))) add(m[1]);
  }
  for (const re of [FUNC_PARAMS, ARROW_PARAMS, BARE_PARAM]) {
    re.lastIndex = 0;
    while ((m = re.exec(code))) add(m[1]);
  }
  return names;
}

function usedNames(code) {
  const uses = new Map();
  const re = /([.$]?)\b([A-Za-z_$][\w$]*)\b/g;
  let m;
  while ((m = re.exec(code))) {
    const [full, prefix, name] = m;
    if (prefix === "." || prefix === "$") continue; // a property, not a binding
    const before = code.slice(0, m.index).trimEnd().slice(-1);
    if (before === ".") continue; // also a property
    const after = code.slice(m.index + full.length).trimStart();
    // An object literal key: `{ foo: 1 }` or a label.
    if (after.startsWith(":") && !after.startsWith("::")) {
      const prev = code.slice(0, m.index).trimEnd().slice(-1);
      if (prev === "{" || prev === "," || prev === "") continue;
    }
    // A quoted-ish property in `"key": value` is already stripped.
    if (!uses.has(name)) uses.set(name, code.slice(0, m.index).split("\n").length);
  }
  return uses;
}

let bad = 0;
for (const file of process.argv.slice(2)) {
  const source = readFileSync(file, "utf8");
  const code = strip(source);
  const declared = declaredNames(code);
  const uses = usedNames(code);

  const missing = [];
  for (const [name, line] of uses) {
    if (declared.has(name)) continue;
    // A name only ever used as an object key on the right of a dot is fine; but
    // `foo.bar = 1` where `foo` is undeclared is exactly the bug we are hunting.
    missing.push({ name, line });
  }

  if (missing.length) {
    bad += missing.length;
    console.error(`${file}: ${missing.length} undeclared name(s)`);
    for (const { name, line } of missing) {
      const text = source.split("\n")[line - 1].trim();
      console.error(`  ${file}:${line}  ${name}  ${text.slice(0, 90)}`);
    }
  } else {
    console.log(`${file}: no undeclared names (${uses.size} read, ${declared.size} known)`);
  }
}

process.exit(bad ? 1 : 0);
