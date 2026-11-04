// Demo wiring: load the wasm-pack `--target web` build from ./pkg/
// and hook up the Transpile / Run buttons. No framework, no bundler.
import init, { compile, run, version } from "./pkg/c2bf_wasm.js";

const srcEl = document.getElementById("src");
const inputEl = document.getElementById("input");
const sampleEl = document.getElementById("sample");

// Sample programs, one per supported feature area. The sources are
// duplicated in rs/wasm/smoke.cjs, which asserts each one transpiles
// and runs to its expected output; keep the two copies in sync.
const SAMPLES = {
  arithmetic: {
    src: "int main() {\n  printf(\"%d\\n\", 6 * 7);\n}\n",
    input: "",
  },
  loop: {
    // Bare declarators (`int n;`) are rejected: every declaration
    // needs an initializer.
    src: "int main() {\n  int n = 0;\n  int s = 0;\n  int i = 1;\n  scanf(\"%d\", &n);\n  while (i <= n) {\n    s += i;\n    i++;\n  }\n  printf(\"%d\\n\", s);\n}\n",
    input: "5\n",
  },
  strings: {
    src: "int main() {\n  char s[6] = \"hello\";\n  printf(\"%s, %s!\\n\", s, \"world\");\n}\n",
    input: "",
  },
  functions: {
    src: "int square(int x) {\n  return x * x;\n}\n\nint main() {\n  printf(\"%d\\n\", square(9));\n}\n",
    input: "",
  },
  pointers: {
    src: "int main() {\n  int x = 41;\n  int *p = &x;\n  *p = *p + 1;\n  printf(\"%d\\n\", x);\n}\n",
    input: "",
  },
};

function loadSample() {
  const sample = SAMPLES[sampleEl.value];
  srcEl.value = sample.src;
  inputEl.value = sample.input;
  bfEl.textContent = "(nothing transpiled yet)";
  outEl.textContent = "(nothing run yet)";
}

sampleEl.addEventListener("change", loadSample);
const bfEl = document.getElementById("bf");
const outEl = document.getElementById("out");
const statusEl = document.getElementById("status");
const compileBtn = document.getElementById("compile");
const runBtn = document.getElementById("run");

loadSample();

const decoder = new TextDecoder();

function showError(err) {
  statusEl.textContent = `Error: ${err}`;
  statusEl.classList.add("error");
}

function transpile() {
  const bf = compile(srcEl.value);
  bfEl.textContent = bf;
  statusEl.textContent = `Transpiled ok (c2bf ${version()}).`;
  statusEl.classList.remove("error");
  return bf;
}

compileBtn.addEventListener("click", () => {
  try {
    transpile();
  } catch (err) {
    showError(err);
  }
});

runBtn.addEventListener("click", () => {
  try {
    const bf = transpile();
    const input = new TextEncoder().encode(inputEl.value);
    const output = run(bf, input);
    outEl.textContent = decoder.decode(output);
    statusEl.textContent = `Ran ok (c2bf ${version()}).`;
    statusEl.classList.remove("error");
  } catch (err) {
    showError(err);
  }
});

compileBtn.disabled = true;
runBtn.disabled = true;
try {
  await init();
  compileBtn.disabled = false;
  runBtn.disabled = false;
  statusEl.textContent = `Ready (c2bf ${version()}).`;
} catch (err) {
  showError(`failed to load WebAssembly: ${err}`);
}
