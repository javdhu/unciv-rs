#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# Fetch Unciv's "Civ V - Gods & Kings" and "Civ V - Vanilla" rulesets into
# crates/civ-core/rules/gods-and-kings and crates/civ-core/rules/vanilla.
#
# The engine's test suite and its determinism fingerprint use these two
# rulesets as fixtures. Their names and texts derive from Civilization V, so
# this repository does not redistribute them: they are downloaded from
# upstream Unciv (https://github.com/yairm210/Unciv) at a pinned commit and
# checked against the git blob ids recorded below. The game content this
# project ships is rules/base and rules/blitz.
#
# Usage: crates/civ-core/tools/fetch_unciv_rulesets.sh   (needs curl and git)
set -euo pipefail
cd "$(dirname "$0")/.."

UNCIV_COMMIT=ebd7d476ded6afcfc3d6c2bbcb3f1f9f9367214d   # Unciv master, 2026-09-23
BASE="https://raw.githubusercontent.com/yairm210/Unciv/$UNCIV_COMMIT/android/assets/jsons"

# local dir | upstream dir (URL-encoded) | file | git blob id
MANIFEST="
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Beliefs.json|dbd9b5193c9b766361efd4a8032e88d493e18525
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Buildings.json|1335494318d59878724faea20430c459d2539aae
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|CityStateTypes.json|12cffb5d420ce88f56a500fcbb9db492134d11e7
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Difficulties.json|59eda00e8d171940721c5222a76891b1bcc797ac
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Eras.json|20870b85f429248976b4f06cd337d5c7e5317d24
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Events.json|3ae4ef2126d335517cd41ed5b5fae87275416f3e
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|GlobalUniques.json|d2fae33ac6e76828a8d413d2cc619230b3594e83
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|ModOptions.json|319c0aad9b44260526342aaca0450db1f295f195
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Nations.json|ca3e3565e01bfa98de29e9390e9a4c50f9f52fef
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Personalities.json|f2b9f95275a07fff17890ea03160f93fd535a9fe
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Policies.json|dd8c7feccdb966f9e1938a53be9108c87a90f777
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Quests.json|016e8127dd7c3c8f3ffc2fe3462466e91dd067ce
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Religions.json|5c7c176e00cbb30da84eb841929fc4ebf13494f9
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Ruins.json|11c5a495e67e17fe03dccbc6dd8f395178a591dd
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Specialists.json|9ac72274fc276004569f49196d5bd550cfeb942b
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Speeds.json|75aabcd708f1b1102c86f60d039e69c4c4f77aaa
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Techs.json|c1a9dafe0d4208858d76da6ba1e69c53829fc878
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Terrains.json|cb1c21a2c7a35966bc8aad983cd6948697dc9e52
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|TileImprovements.json|3aa17684885e5c7a57424c0155216ec9dbeb23f9
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|TileResources.json|48d77ed3f68cbe69b03161e97396db537333062d
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|UnitNameGroups.json|d6b515127b7d6fb5592a2d2b2d20230058a8b5ab
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|UnitPromotions.json|070d930b477d1cac000a585e97076b133a04d1e1
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|UnitTypes.json|3b52fc771c18d8d856e2a5900befbe9c9ffcda69
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|Units.json|70fc5f63c340575fd3ec6b8baf6c78a9b9655328
gods-and-kings|Civ%20V%20-%20Gods%20%26%20Kings|VictoryTypes.json|7f69325701bbcc61b324b5e16bc6861caf33ce27
vanilla|Civ%20V%20-%20Vanilla|Buildings.json|047c2eacb8f50ae037a23a0ac8f573740b70438c
vanilla|Civ%20V%20-%20Vanilla|CityStateTypes.json|0d21a246192b382f14c6a78fbedb954d9453cc41
vanilla|Civ%20V%20-%20Vanilla|Difficulties.json|ee28ce7daab0b187f9121a9656d94a2fcc3d44c8
vanilla|Civ%20V%20-%20Vanilla|Eras.json|ad8bf5dcb77bff04dc6e46fe4327ca3aba92417b
vanilla|Civ%20V%20-%20Vanilla|GlobalUniques.json|8df7c64970189d2448a7282aedaade32270b78c0
vanilla|Civ%20V%20-%20Vanilla|ModOptions.json|be164bf27860c8a7a54801d5716781a22f70c493
vanilla|Civ%20V%20-%20Vanilla|Nations.json|06922f383c739580a685a92a0534a850c155b96f
vanilla|Civ%20V%20-%20Vanilla|Policies.json|c547ee7b8bd23ad23a1c1174bf8af69668049b42
vanilla|Civ%20V%20-%20Vanilla|Quests.json|5ebb35bce982ca126d0553e9456d4ce4ecf6dd75
vanilla|Civ%20V%20-%20Vanilla|Ruins.json|1471d132a2b61beb04fcca0820253a152143f395
vanilla|Civ%20V%20-%20Vanilla|Specialists.json|9ac72274fc276004569f49196d5bd550cfeb942b
vanilla|Civ%20V%20-%20Vanilla|Speeds.json|9c6aeba13da06fa498c6eb365e40e47b260029c5
vanilla|Civ%20V%20-%20Vanilla|Techs.json|11555236d5b00e1751f25200c2ae1b3742c57111
vanilla|Civ%20V%20-%20Vanilla|Terrains.json|090bcbd462a37748ff4abe08cab00755e10b766c
vanilla|Civ%20V%20-%20Vanilla|TileImprovements.json|31060fa01057cf54fabbece127ce5c118b029802
vanilla|Civ%20V%20-%20Vanilla|TileResources.json|b75a52d986ba3c7de544edb7146a459d55840eaa
vanilla|Civ%20V%20-%20Vanilla|UnitPromotions.json|933a4038a9bda4e20c24db244082da92f68afc7e
vanilla|Civ%20V%20-%20Vanilla|UnitTypes.json|3b52fc771c18d8d856e2a5900befbe9c9ffcda69
vanilla|Civ%20V%20-%20Vanilla|Units.json|a98add326a8a5ead18707536591f7a8b8a61e9db
vanilla|Civ%20V%20-%20Vanilla|VictoryTypes.json|7f69325701bbcc61b324b5e16bc6861caf33ce27
"

fail=0
while IFS="|" read -r dir upstream file blob; do
  [ -n "$dir" ] || continue
  dest="rules/$dir/$file"
  mkdir -p "rules/$dir"
  if [ -f "$dest" ] && [ "$(git hash-object "$dest")" = "$blob" ]; then
    continue
  fi
  curl -fsSL --retry 3 -o "$dest.tmp" "$BASE/$upstream/$file"
  got=$(git hash-object "$dest.tmp")
  if [ "$got" != "$blob" ]; then
    echo "checksum mismatch: $dest (expected $blob, got $got)" >&2
    rm -f "$dest.tmp"
    fail=1
    continue
  fi
  mv "$dest.tmp" "$dest"
done <<< "$MANIFEST"
[ "$fail" = 0 ] || exit 1
echo "Unciv rulesets at $UNCIV_COMMIT: rules/gods-and-kings, rules/vanilla"
