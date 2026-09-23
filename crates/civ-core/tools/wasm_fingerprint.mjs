// Loads the wasm build and prints its fingerprint, for comparison with the
// native one. The whole determinism claim of this crate reduces to these two
// numbers matching.
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";

// Cargo's output may be redirected by CARGO_TARGET_DIR or local configuration.
// Ask Cargo rather than accidentally checking an older project-local build.
function cargoWasmPath() {
  const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
  }));
  return join(metadata.target_directory, "wasm32-unknown-unknown", "release", "civ_core.wasm");
}

const path = process.argv[2] ?? cargoWasmPath();
const bytes = readFileSync(path);

let instance;
try {
  ({ instance } = await WebAssembly.instantiate(bytes, {}));
} catch (e) {
  // Report what the module wants, rather than just failing.
  const mod = await WebAssembly.compile(bytes);
  const imports = WebAssembly.Module.imports(mod);
  console.error("instantiation failed:", e.message);
  if (imports.length) console.error("module imports:", imports);
  process.exit(1);
}

const fn = instance.exports.civ_core_fingerprint;
if (typeof fn !== "function") {
  console.error("export civ_core_fingerprint missing; exports are:",
    Object.keys(instance.exports));
  process.exit(1);
}

const value = fn();
// u64 arrives as a BigInt; print it the same way Rust's {:#018x} does.
console.log("0x" + BigInt.asUintN(64, value).toString(16).padStart(16, "0"));
