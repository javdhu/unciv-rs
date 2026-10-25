#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""Generate the wire contract (Rust + TypeScript) from Unciv's apiv2 Kotlin structs.

One source of truth for both ends of the socket, so a field rename cannot
silently desync client and server.
"""
import re, json, pathlib

import os, sys
CRATE = pathlib.Path(__file__).resolve().parent.parent
if "UNCIV_DIR" not in os.environ:
    sys.exit("set UNCIV_DIR to an Unciv checkout (https://github.com/yairm210/Unciv)")
ROOT = pathlib.Path(os.environ["UNCIV_DIR"])
MPL = ("// This Source Code Form is subject to the terms of the Mozilla Public\n"
       "// License, v. 2.0. If a copy of the MPL was not distributed with this\n"
       "// file, You can obtain one at https://mozilla.org/MPL/2.0/.\n\n")
API    = ROOT/"core/src/com/unciv/logic/multiplayer/apiv2"
RS_OUT = CRATE/"src/generated"
# The TypeScript contract is written only when TS_OUT names a directory.
TS_OUT = pathlib.Path(os.environ["TS_OUT"]) if os.environ.get("TS_OUT") else None
for d in (RS_OUT, TS_OUT):
    if d: d.mkdir(parents=True, exist_ok=True)

SCALARS = {  # kotlin -> (rust, ts)
    "String": ("String", "string"), "Int": ("i32", "number"), "Long": ("i64", "number"),
    "Short": ("i16", "number"), "Byte": ("i8", "number"), "UInt": ("u32", "number"),
    "ULong": ("u64", "number"), "Boolean": ("bool", "boolean"), "Float": ("f32", "number"),
    "Double": ("f64", "number"), "UUID": ("Uuid", "string"), "Instant": ("Timestamp", "string"),
    "LocalDateTime": ("Timestamp", "string"), "Duration": ("i64", "number"),
}

def map_type(kt):
    kt = kt.strip()
    opt = kt.endswith("?")
    if opt: kt = kt[:-1].strip()
    m = re.fullmatch(r'(List|Set|Array|MutableList|MutableSet)<(.+)>', kt)
    if m:
        r, t = map_type(m.group(2))[:2]
        rs, ts = f"Vec<{r}>", f"{t}[]"
    else:
        m = re.fullmatch(r'(Map|MutableMap|HashMap)<([^,]+),(.+)>', kt)
        if m:
            kr, kts = map_type(m.group(2))[:2]
            vr, vts = map_type(m.group(3))[:2]
            rs, ts = f"std::collections::HashMap<{kr}, {vr}>", f"Record<{kts}, {vts}>"
        else:
            rs, ts = SCALARS.get(kt, (kt, kt))
    if opt: rs, ts = f"Option<{rs}>", f"{ts} | null"
    return rs, ts, opt

RUST_KW = {"type","match","move","ref","self","fn","impl","mod","use","crate","super",
           "where","box","final","loop","struct","enum","trait","let","const","static",
           "as","in","if","else","for","while","return","true","false","pub","unsafe"}

def rs_ident(s):
    return f"r#{s}" if s in RUST_KW else s

def snake(s):
    s = re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '_', s)
    return re.sub(r'_+', '_', s).lower()

# --- parse -------------------------------------------------------------------
def find_classes(src):
    """Balanced-paren scan: regexes mis-handle `) : Base()` and defaults like `= listOf()`."""
    out = []
    for m in re.finditer(r'@Serializable(?:\([^)]*\))?\s*(?:data\s+)?class\s+([A-Za-z0-9_]+)\s*\(', src):
        i, depth = m.end() - 1, 0
        while i < len(src):
            if src[i] == '(': depth += 1
            elif src[i] == ')':
                depth -= 1
                if depth == 0: break
            i += 1
        body = src[m.end():i]
        doc = ""
        pre = src[max(0, m.start() - 600):m.start()]
        d = re.findall(r'/\*\*(.*?)\*/', pre, re.S)
        if d and pre.rstrip().endswith(('*/', '/')) is False:
            doc = d[-1] if pre[pre.rfind('*/') + 2:].strip().startswith('@Serializable') else ""
        elif d:
            doc = d[-1]
        out.append((m.group(1), body, doc))
    return out

def strip_comments(body):
    body = re.sub(r'/\*.*?\*/', '', body, flags=re.S)
    return "\n".join(re.sub(r'//.*$', '', ln) for ln in body.splitlines())

def split_fields(body):
    body = strip_comments(body)
    """Split on commas that are outside <>, () and strings."""
    parts, cur, ang, par, instr = [], [], 0, 0, False
    for c in body:
        if instr:
            cur.append(c)
            if c == '"': instr = False
            continue
        if c == '"': instr = True
        elif c == '<': ang += 1
        elif c == '>': ang -= 1
        elif c == '(': par += 1
        elif c == ')': par -= 1
        elif c == ',' and ang == 0 and par == 0:
            parts.append(''.join(cur)); cur = []; continue
        cur.append(c)
    if ''.join(cur).strip(): parts.append(''.join(cur))
    return parts
ENUM = re.compile(r'enum class\s+([A-Za-z0-9_]+)\s*(?:\(([^)]*)\))?\s*\{(.*?)^\}', re.S | re.M)
VARIANT = re.compile(r'^\s{4}([A-Z][A-Za-z0-9_]*)\s*(?:\(([^)]*)\))?\s*[,;]', re.M)

structs, enums = [], []
for f in sorted(API.glob("*.kt")):
    src = f.read_text()
    for m in ENUM.finditer(src):
        vals = []
        for e in VARIANT.finditer(m.group(3)):
            name, args = e.group(1), (e.group(2) or "")
            ws = re.search(r'"([^"]*)"', args)
            vals.append((name, ws.group(1) if ws else name))
        if vals: enums.append((m.group(1), vals))
    for name, body, doc in find_classes(src):
        fields = []
        for raw in split_fields(body):
            raw = raw.strip()
            if not raw: continue
            wire = re.search(r'@SerialName\("([^"]+)"\)', raw)
            fm = re.search(r'(?:val|var)\s+([A-Za-z0-9_]+)\s*:\s*([^=]+?)(?:\s*=\s*(.+))?$', raw, re.S)
            if not fm: continue
            fname, ftype = fm.group(1), fm.group(2).strip()
            fields.append((fname, wire.group(1) if wire else fname, ftype, fm.group(3) is not None))
        if fields:
            structs.append((name, (doc or "").strip().replace("*","").strip(), fields, f.stem))

# --- emit Rust ---------------------------------------------------------------
L = ["// @generated by tools/generate_api.py from logic/multiplayer/apiv2/*.kt — do not edit.",
     "//! Wire contract shared by the server and the wasm client.",
     "#![allow(dead_code)]", "",
     "/// Opaque UUID; kept as a string so the core stays dependency-free.",
     "pub type Uuid = String;",
     "/// RFC-3339 timestamp, likewise opaque.",
     "pub type Timestamp = String;", "",
]
for name, doc, fields, origin in structs:
    if doc: L += [f"/// {l.strip()}" for l in doc.splitlines() if l.strip()]
    L.append(f"/// (from `{origin}.kt`)")
    L.append('#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]')
    L.append("#[derive(Debug, Clone, PartialEq)]")
    L.append(f"pub struct {name} {{")
    for fname, wire, ftype, _ in fields:
        rs = map_type(ftype)[0]
        sn = snake(fname)
        if sn != wire:
            L.append(f'    #[cfg_attr(feature = "serde", serde(rename = "{wire}"))]')
        L.append(f"    pub {rs_ident(sn)}: {rs},")
    L += ["}", ""]
for name, vals in enums:
    L.append('#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]')
    L.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
    L.append(f"pub enum {name} {{")
    for v, wire in vals:
        if v != wire:
            L.append(f'    #[cfg_attr(feature = "serde", serde(rename = "{wire}"))]')
        L.append(f"    {v},")
    L += ["}", ""]
(RS_OUT/"api.rs").write_text(MPL + "\n".join(L) + "\n")

# --- emit TypeScript ---------------------------------------------------------
T = ["// @generated by tools/generate_api.py from logic/multiplayer/apiv2/*.kt — do not edit.",
     "", "export type Uuid = string;", "export type Timestamp = string;", ""]
for name, doc, fields, origin in structs:
    if doc: T += ["/**"] + [f" * {l.strip()}" for l in doc.splitlines() if l.strip()] + [" */"]
    T.append(f"export interface {name} {{")
    for fname, wire, ftype, has_default in fields:
        ts, opt = map_type(ftype)[1], map_type(ftype)[2]
        T.append(f"  {wire}{'?' if has_default else ''}: {ts};")
    T += ["}", ""]
for name, vals in enums:
    T.append(f"export type {name} = " + " | ".join(json.dumps(w) for _, w in vals) + ";")
if TS_OUT: (TS_OUT/"api.ts").write_text("\n".join(T) + "\n")

print(f"structs  {len(structs)}")
print(f"enums    {len(enums)}")
print(f"fields   {sum(len(f) for _,_,f,_ in structs)}")
