#!/usr/bin/env python3
"""Generate ruleset structs from the shipped json.

The ruleset is data, not code — its schema is inferable from what actually
ships, across both bundled rulesets, so these types are derived rather than
invented. Numbers become fixed-point: the rules layer must not touch floats.
"""
import json, re, pathlib, collections

ROOT = pathlib.Path("../Unciv")
RS   = ROOT/"android/assets/jsons"
OUT  = ROOT/"civ-core-rs/src/ruleset"
OUT.mkdir(parents=True, exist_ok=True)

def relax(t):
    out,i,n,instr=[],0,len(t),False
    while i<n:
        c=t[i]
        if instr:
            out.append(c)
            if c=='\\' and i+1<n: out.append(t[i+1]); i+=2; continue
            if c=='"': instr=False
            i+=1; continue
        if c=='"': instr=True; out.append(c); i+=1; continue
        if c=='/' and i+1<n and t[i+1]=='/':
            while i<n and t[i]!='\n': i+=1
            continue
        if c=='/' and i+1<n and t[i+1]=='*':
            i+=2
            while i+1<n and not(t[i]=='*' and t[i+1]=='/'): i+=1
            i+=2; continue
        out.append(c); i+=1
    return re.sub(r',(\s*[}\]])', r'\1', ''.join(out))

# file -> (outer struct, key holding the inner list, inner struct)
NESTED = {
    "Techs.json":    ("TechColumn", "techs",    "Technology"),
    "Policies.json": ("PolicyBranch", "policies", "Policy"),
}
SINGLE = {
    "Buildings.json":"Building", "Units.json":"BaseUnit", "Terrains.json":"Terrain",
    "Nations.json":"Nation", "TileImprovements.json":"TileImprovement",
    "TileResources.json":"TileResource", "UnitPromotions.json":"Promotion",
    "Beliefs.json":"Belief", "Difficulties.json":"Difficulty", "Eras.json":"Era",
    "Speeds.json":"Speed", "Specialists.json":"Specialist", "Personalities.json":"Personality",
    "VictoryTypes.json":"Victory", "Quests.json":"Quest", "Ruins.json":"RuinReward",
    "CityStateTypes.json":"CityStateType", "UnitTypes.json":"UnitType", "Events.json":"Event",
}
# Relationships Unciv establishes while loading rather than reading. They exist
# as json *structure* (a tech's column is its nesting) or as position (an era's
# number is its index), so inferring the schema from the data cannot see them.
# Populated by `ruleset/load.rs`, never by `from_json`.
DERIVED = {
    "Technology": [("column", "Option<Id<TechColumn>>",
                    "The column this tech sits in. Nesting in Techs.json, not a field.")],
    "Era":        [("era_number", "i32",
                    "Position in Eras.json. Unciv assigns this at load (Ruleset.kt:425).")],
    "Policy":     [("branch", "Option<Id<PolicyBranch>>",
                    "The branch this policy belongs to (Ruleset.kt:468).")],
    "Speed":      [("total_turns", "i32",
                    "Last `untilTurn` in the `turns` array (Speed.kt numTotalTurns). "
                    "The array is nested structure, so schema inference reads it as opaque."),
                   ("year_intervals", "Vec<(Fixed, i32)>", "Parsed calendar intervals: years per turn and inclusive end turn.")],
    "Terrain":    [("damage_per_turn", "i32",
                    "Summed from DamagesContainingUnits uniques (Terrain.kt:212).")],
}

STAT_FIELDS = ["food","production","gold","science","culture","happiness","faith"]

# Unciv declares these as `RulesetStatsObject`, meaning all seven yields are
# always available. The shipped data sets only some — no improvement grants
# happiness — so inferring from the data alone would omit fields the format
# permits and a mod could use. Forced present.
STATS_OBJECTS = {"Building", "TileImprovement", "TileResource", "Terrain"}

RUST_KW = {"type","match","move","ref","self","fn","impl","mod","use","crate","super","where",
           "box","final","loop","struct","enum","trait","let","const","static","as","in","if",
           "else","for","while","return","true","false","pub","unsafe","abstract","become",
           "override","priv","macro","yield","await","async","dyn","virtual","try"}

def snake(s):
    s = re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '_', s)
    s = re.sub(r'(?<=[A-Z])(?=[A-Z][a-z])', '_', s)
    s = re.sub(r'[^0-9A-Za-z]+','_',s).strip('_').lower()
    return f"r#{s}" if s in RUST_KW else s

def collect(o, want_key=None):
    """Named objects at the top level, or inside `want_key` lists."""
    found = []
    def walk(v, inside):
        if isinstance(v, dict):
            if "name" in v and inside: found.append(v)
            for k, x in v.items():
                walk(x, k == want_key if want_key else inside)
        elif isinstance(v, list):
            for x in v: walk(x, inside)
    walk(o, want_key is None)
    return found

def kinds(objs):
    acc = collections.defaultdict(collections.Counter)
    for o in objs:
        for k, v in o.items():
            if isinstance(v, bool): t="bool"
            elif isinstance(v, int): t="int"
            elif isinstance(v, float): t="num"
            elif isinstance(v, str): t="str"
            elif isinstance(v, list):
                inner = {("int" if isinstance(x,int) and not isinstance(x,bool)
                          else "num" if isinstance(x,float) else "str" if isinstance(x,str)
                          else "obj") for x in v}
                t = "vec_int" if inner <= {"int"} and inner else "vec_str" if inner <= {"str"} else "vec_obj" if inner else "vec_str"
            elif isinstance(v, dict): t="counter"
            else: t="str"
            acc[k][t]+=1
    return acc

RUST = {"bool":"bool","int":"i32","num":"Fixed","str":"String",
        "vec_int":"Vec<i32>","vec_str":"Vec<String>","vec_obj":"Vec<Todo>",
        "counter":"std::collections::BTreeMap<String, i32>"}

def rust_type(ks):
    # int+num together means the json writes both `1` and `1.2` — fixed-point covers both
    s = set(ks)
    if s == {"int"}: return "i32"
    if s <= {"int","num"}: return "Fixed"
    if "str" in s and len(s) > 1: return "String"
    return RUST[sorted(s, key=lambda k:-ks[k])[0]]

files = collections.defaultdict(list)
for base in ["Civ V - Gods & Kings", "Civ V - Vanilla"]:
    for f in sorted((RS/base).glob("*.json")):
        try: files[f.name].append(json.loads(relax(f.read_text(encoding="utf-8-sig"))))
        except Exception: pass

structs = []
def build(struct, objs, note):
    ks = kinds(objs)
    fields = []
    for k in sorted(ks, key=lambda k: (-sum(ks[k].values()), k)):
        if k == "name": continue
        present = sum(ks[k].values())
        ty = "std::collections::BTreeMap<String, Fixed>" if k == "weightForCityStateType" else rust_type(ks[k])
        # An absent list or table is an empty one; `Option<Vec<_>>` would make
        # every reader distinguish two things that mean the same.
        opt = present < len(objs) and not (ty.startswith("Vec<") or ty.startswith("std::collections::"))
        fields.append((k, ty, opt, present, len(objs)))
    # RuinReward.kt permits custom reward weights even though the bundled
    # rulesets rely on its default of one.
    if struct == "RuinReward" and "weight" not in ks:
        fields.append(("weight", "i32", True, 0, len(objs)))
    structs.append((struct, note, len(objs), fields))

for fname, (outer, key, inner) in NESTED.items():
    docs = files.get(fname, [])
    inner_objs = [o for d in docs for o in collect(d, key)]
    outer_objs = [o for d in docs for o in (d if isinstance(d, list) else [])]
    build(inner, inner_objs, f"{fname} (nested under `{key}`)")
    if outer_objs: build(outer, [{k:v for k,v in o.items() if k != key} for o in outer_objs], f"{fname} (outer)")
for fname, struct in SINGLE.items():
    objs = [o for d in files.get(fname, []) for o in collect(d)]
    if objs: build(struct, objs, fname)

L = ["// @generated by tools/generate_ruleset.py from android/assets/jsons — do not edit.",
     "//! Ruleset data types, inferred from what the shipped rulesets actually contain.",
     "//!",
     "//! Every field is optional in the json except `name`, so anything not present on",
     "//! all objects is an `Option`. Percentages and multipliers are [`Fixed`], never",
     "//! floats — the rules layer has to be bit-identical between native and wasm.",
     "#![allow(dead_code)]", "",
     "use crate::stats::Fixed;",
     "use crate::unique::Unique;",
     "use super::Unstructured as Todo;",
     "use crate::json::Json;",
     "use crate::arena::Id;", ""]
for struct, note, n, fields in structs:
    L.append(f"/// From `{note}` — {n} objects across both shipped rulesets.")
    L.append("#[derive(Debug, Clone, Default, PartialEq)]")
    L.append(f"pub struct {struct} {{")
    L.append("    pub name: String,")
    for k, ty, opt, present, total in fields:
        if k == "uniques":
            L.append(f"    /// Present on {present}/{total}; parsed at load.")
            L.append("    pub uniques: Vec<Unique>,")
            continue
        L.append(f"    /// {present}/{total} objects carry this.")
        L.append(f"    pub {snake(k)}: {f'Option<{ty}>' if opt else ty},")
    existing = {snake(k) for k, *_ in fields} | {"name"}
    if struct in STATS_OBJECTS:
        for st in STAT_FIELDS:
            if st not in existing:
                L.append(f"    /// Unused by the shipped rulesets; a `RulesetStatsObject` field a mod may set.")
                L.append(f"    pub {st}: Option<i32>,")
                existing.add(st)
    for fname, fty, why in DERIVED.get(struct, []):
        assert fname not in existing, f"{struct}.{fname} would shadow a json field"
        L.append(f"    /// {why}")
        L.append("    ///")
        L.append("    /// Derived at load: absent from the json, so `from_json` never sets it.")
        L.append(f"    pub {fname}: {fty},")
    L += ["}", ""]
# from_json: the schema already knows every field's json key and type, so the
# reader is derived from the same source as the struct rather than kept in sync.
READ = {
    "i32": "as_i32()", "Fixed": "as_fixed()", "bool": "as_bool()",
    "String": "as_str().map(String::from)",
}
L += ["impl Ruleset {", "    // marker", "}", ""]
L = L[:-4]
for struct, note, n, fields in structs:
    L.append(f"impl {struct} {{")
    L.append("    /// Read one object. Absent fields take their default; the ruleset")
    L.append("    /// json omits far more than it states.")
    L.append("    pub fn from_json(j: &Json) -> Option<Self> {")
    L.append('        // Tech columns carry no `name`; the loader supplies one.')
    L.append('        let name = j.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();')
    L.append("        Some(Self {")
    L.append("            name,")
    for k, ty, opt, present, total in fields:
        g = f'j.get("{k}")'
        if k == "uniques":
            L.append(f'            uniques: {g}.map(|v| v.strings().iter().map(|s| Unique::parse(s)).collect()).unwrap_or_default(),')
            continue
        fld = snake(k)
        if ty == "Vec<String>":
            expr = f'{g}.map(|v| v.strings()).unwrap_or_default()'
        elif ty == "Vec<i32>":
            expr = f'{g}.map(|v| v.ints()).unwrap_or_default()'
        elif ty == "std::collections::BTreeMap<String, Fixed>":
            expr = f'{g}.and_then(|v|v.as_object()).map(|m|m.iter().filter_map(|(k,v)|v.as_fixed().map(|n|(k.clone(),n))).collect()).unwrap_or_default()'
        elif ty.startswith("std::collections::BTreeMap"):
            expr = f'{g}.map(|v| v.counter()).unwrap_or_default()'
        elif ty == "Vec<Todo>":
            expr = "Default::default()"
        else:
            inner = f'{g}.and_then(|v| v.{READ[ty]})'
            expr = inner if opt else f'{inner}.unwrap_or_default()'
        L.append(f"            {fld}: {expr},")
    if struct in STATS_OBJECTS:
        for st in STAT_FIELDS:
            if st not in {snake(k) for k, *_ in fields}:
                L.append(f'            {st}: j.get("{st}").and_then(|v| v.as_i32()),')
    for fname, _, _ in DERIVED.get(struct, []):
        L.append(f"            {fname}: Default::default(), // set by ruleset/load.rs")
    L += ["        })", "    }", "}", ""]
(OUT/"generated.rs").write_text("\n".join(L) + "\n")

print(f"structs  {len(structs)}")
for s, note, n, f in structs: print(f"   {s:<18} {len(f):>2} fields  ({n} objects)")
