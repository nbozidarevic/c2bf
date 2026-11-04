// Node smoke test for the wasm-pack `--target nodejs` build.
// Usage: node rs/wasm/smoke.cjs <path-to-built-dir>
// The built dir contains c2bf_wasm.js (CommonJS) next to its .wasm.
// Exits nonzero on the first failed assertion.
"use strict";

const assert = require("node:assert/strict");
const path = require("node:path");

const dir = process.argv[2];
if (!dir) {
  console.error("usage: node smoke.cjs <built-dir>");
  process.exit(2);
}
const { compile, run, format_code, version } = require(path.join(dir, "c2bf_wasm.js"));

assert.match(version(), /^\d+\.\d+\.\d+$/, "version looks like semver");

// The demo page samples (demo/demo.js SAMPLES), duplicated here so
// CI proves each one transpiles and runs to its expected output.
// Keep the two copies in sync.
const samples = [
  { src: 'int main() { printf("%d\\n", 6 * 7); }', input: "", out: [52, 50, 10] },
  {
    src: "int main() { int n = 0; int s = 0; int i = 1; scanf(\"%d\", &n); while (i <= n) { s += i; i++; } printf(\"%d\\n\", s); }",
    input: "5\n",
    out: [49, 53, 10],
  },
  {
    src: 'int main() { char s[6] = "hello"; printf("%s, %s!\\n", s, "world"); }',
    input: "",
    out: [104, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33, 10],
  },
  {
    src: "int square(int x) { return x * x; } int main() { printf(\"%d\\n\", square(9)); }",
    input: "",
    out: [56, 49, 10],
  },
  {
    src: "int main() { int x = 41; int *p = &x; *p = *p + 1; printf(\"%d\\n\", x); }",
    input: "",
    out: [52, 50, 10],
  },
];
for (const sample of samples) {
  const bf = compile(sample.src);
  assert.equal(typeof bf, "string");
  assert.ok(bf.endsWith("\n"), "compile output ends with newline");
  const out = run(bf, Buffer.from(sample.input));
  assert.ok(out instanceof Uint8Array, "run returns bytes");
  assert.deepEqual(Array.from(out), sample.out, `sample prints ${JSON.stringify(sample.out)}`);
}

assert.throws(
  () => compile("int main( {"),
  (err) => typeof err === "string" && err.length > 0,
  "bad source throws a diagnostic",
);
assert.throws(
  () => run("]", new Uint8Array(0)),
  /unexpected/,
  "unbalanced close throws",
);

assert.equal(format_code("a + b [ignored]", true), "+[]\n", "minify keeps commands");

console.log("wasm smoke ok, version", version());
