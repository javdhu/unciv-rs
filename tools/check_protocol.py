#!/usr/bin/env python3
"""Verify committed client types and runtime schemas against their Rust source."""
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess

root = Path(__file__).resolve().parents[1]
checked = root / 'web/src/engine/generated'
with TemporaryDirectory(prefix='civ-protocol-') as temp:
    generated = Path(temp)
    subprocess.run(['cargo', 'run', '--quiet', '-p', 'civ-sim', '--example', 'export_protocol', '--', str(generated)], cwd=root, check=True)
    expected = {p.name: p.read_bytes() for p in generated.iterdir() if p.is_file()}
    actual = {p.name: p.read_bytes() for p in checked.iterdir() if p.is_file()}
    different = sorted(name for name in expected.keys() | actual.keys() if expected.get(name) != actual.get(name))
    if different:
        raise SystemExit('Generated protocol differs: ' + ', '.join(different) + '\nRun cargo run -p civ-sim --example export_protocol')
    print('Rust and browser protocol definitions match.')
