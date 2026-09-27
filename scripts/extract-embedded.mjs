/*
 * Recover a UI file from the compiled client.
 *
 * `assets.rs` pulls every UI file in with `include_str!`, so the exact bytes of the
 * last built copy are still inside the executable. That is the safety net for the
 * one editor mistake this tree keeps inviting: several files here are UTF-8 with
 * CRLF (`main.css`, `pages.js`) and several are UTF-8 with LF (`app.js`,
 * `index.html`), and PowerShell 5.1 decodes a BOM-less UTF-8 file as ANSI. One
 * `Get-Content | ... | Set-Content` round trip through that code page turns every
 * Chinese string into mojibake, and the round trip is lossy, so the bytes cannot be
 * inverted - the last build is the only clean copy left.
 *
 * The layout it relies on: the asset's route string is stored immediately before its
 * contents, so the file starts right after `/pages.js` and runs to the first NUL.
 *
 *   node scripts/extract-embedded.mjs target/debug/hongshi.exe pages.js recovered.js
 *
 * Then diff the result against what you meant to have and move it back into place.
 */
import { readFileSync, writeFileSync } from "node:fs";

const [, , exePath, name, outPath] = process.argv;

if (!exePath || !name || !outPath) {
  console.error("usage: node scripts/extract-embedded.mjs <exe> <name> <out>");
  console.error("  name is the route filename, e.g. pages.js");
  process.exit(2);
}

/* latin1 maps one byte to one char, so offsets and slices stay byte-exact. */
const exe = readFileSync(exePath).toString("latin1");

const route = Buffer.from("/" + name, "utf8").toString("latin1");
const marker = route + Buffer.from("/*", "utf8").toString("latin1");

const at = exe.indexOf(marker);
if (at < 0) {
  console.error(`no embedded copy of ${name} found in ${exePath}`);
  console.error("(the client must have been built with this file already in web/)");
  process.exit(2);
}
if (exe.indexOf(marker, at + 1) >= 0) {
  console.error(`${name}: more than one embedded copy; refusing to guess`);
  process.exit(2);
}

const start = at + route.length;
const end = exe.indexOf("\u0000", start);
if (end < 0) {
  console.error(`${name}: no terminator after the embedded copy`);
  process.exit(2);
}

const bytes = Buffer.from(exe.slice(start, end), "latin1");
writeFileSync(outPath, bytes);

const text = bytes.toString("utf8");
const crlf = (text.match(/\r\n/g) || []).length;
const lone = (text.match(/(?<!\r)\n/g) || []).length;
console.error(
  `${name}: recovered ${bytes.length} bytes to ${outPath} ` +
  `(${text.split("\n").length} lines, CRLF=${crlf} LF=${lone})`
);
if (text.includes("\uFFFD")) {
  console.error("WARNING: replacement characters present - the copy may itself be damaged");
  process.exit(1);
}
