import re, json, pathlib
ROOT = pathlib.Path("../Unciv")
OUT  = pathlib.Path(__file__).resolve().parent.parent
(OUT/"src").mkdir(parents=True, exist_ok=True)
(OUT/"tests").mkdir(parents=True, exist_ok=True)

# resolve Constants.kt string consts so `FreshWater(Constants.freshWater,...)` works
consts = dict(re.findall(r'const val (\w+)\s*=\s*"((?:[^"\\]|\\.)*)"',
                         (ROOT/"core/src/com/unciv/Constants.kt").read_text()))

src = re.sub(r'//[^\n]*', '', (ROOT/"core/src/com/unciv/models/ruleset/unique/UniqueType.kt").read_text())
ENTRY = re.compile(r'^\s{4}([A-Z][A-Za-z0-9_]*)\(\s*(?:"((?:[^"\\]|\\.)*)"|Constants\.(\w+))', re.M)

def unescape(s): return s.replace('\\"','"').replace('\\\\','\\').replace('\\n','\n')
def placeholder(s):
    out, d = [], 0
    for c in s:
        if c == '[':
            if d == 0: out.append('[]')
            d += 1
        elif c == ']': d = max(0, d-1)
        elif d == 0: out.append(c)
    return ''.join(out)

entries, seen = [], set()
for m in ENTRY.finditer(src):
    name = m.group(1)
    if name in seen: continue
    text = unescape(m.group(2)) if m.group(2) is not None else consts.get(m.group(3))
    if text is None: continue
    seen.add(name)
    tail = src[m.end():m.end()+300].split('),')[0]
    entries.append((name, text, sorted(set(re.findall(r'UniqueTarget\.([A-Za-z]+)', tail)))))

targets = sorted({t for _,_,ts in entries for t in ts})
# collisions: two UniqueTypes sharing one placeholder -> first wins, but we must know
ph = {}
dupes = []
for n,t,_ in entries:
    p = placeholder(t)
    if p in ph: dupes.append((ph[p], n, p))
    else: ph[p] = n

def rs(s): return '"' + s.replace('\\','\\\\').replace('"','\\"') + '"'

L = []
L.append("// @generated from core/src/com/unciv/models/ruleset/unique/UniqueType.kt — do not edit by hand.\n")
L.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]")
L.append("pub enum UniqueTarget {")
for t in targets: L.append(f"    {t},")
L.append("}\n")
L.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]")
L.append("pub enum UniqueType {")
for n,_,_ in entries: L.append(f"    {n},")
L.append("}\n")
L.append("impl UniqueType {")
L.append(f"    pub const ALL: [UniqueType; {len(entries)}] = [")
for n,_,_ in entries: L.append(f"        UniqueType::{n},")
L.append("    ];\n")
L.append("    /// The authored template, e.g. `\"[stats] from [tileFilter] tiles [cityFilter]\"`.")
L.append("    pub fn template(self) -> &'static str {")
L.append("        match self {")
for n,t,_ in entries: L.append(f"            UniqueType::{n} => {rs(t)},")
L.append("        }\n    }\n")
L.append("    /// Template with every top-level `[param]` collapsed to `[]` — the match key.")
L.append("    pub fn placeholder(self) -> &'static str {")
L.append("        match self {")
for n,t,_ in entries: L.append(f"            UniqueType::{n} => {rs(placeholder(t))},")
L.append("        }\n    }\n")
L.append("    /// Which ruleset objects may carry this unique.")
L.append("    pub fn targets(self) -> &'static [UniqueTarget] {")
L.append("        match self {")
for n,_,ts in entries:
    L.append(f"            UniqueType::{n} => &[{', '.join('UniqueTarget::'+t for t in ts)}],")
L.append("        }\n    }\n}")
(OUT/"src/unique_type.rs").write_text('\n'.join(L) + '\n')

print(f"generated {len(entries)} UniqueType variants, {len(targets)} targets")
print(f"placeholder collisions: {len(dupes)}")
for a,b,p in dupes[:6]: print(f"   {a} <-> {b}  :: {p[:70]}")
