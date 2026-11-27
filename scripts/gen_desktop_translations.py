#!/usr/bin/env python3
"""Writes the translated Comment[...] and Name[...] lines of packaging/linux/gall.desktop from
packaging/linux/desktop-translations.json (Omarchy's languages). Keeps the English file as the source:
run it after editing either file. `--check` fails when the file is out of date."""
import json
import pathlib
import sys

root = pathlib.Path(__file__).resolve().parent.parent
desktop = root / "packaging/linux/gall.desktop"
tr = json.loads((root / "packaging/linux/desktop-translations.json").read_text(encoding="utf-8"))

out, group = [], ""
for line in desktop.read_text(encoding="utf-8").splitlines():
    if line.startswith(("Comment[", "Name[")):
        continue  # regenerated below
    out.append(line)
    if line.startswith("["):
        group = line
    if line.startswith("Comment=") and group == "[Desktop Entry]":
        out += [f"Comment[{k}]={v[0]}" for k, v in sorted(tr.items())]
    if line == "Name=New Window" and group == "[Desktop Action new-window]":
        out += [f"Name[{k}]={v[1]}" for k, v in sorted(tr.items())]
text = "\n".join(out) + "\n"
if "--check" in sys.argv:
    sys.exit(0 if text == desktop.read_text(encoding="utf-8") else 1)
desktop.write_text(text, encoding="utf-8")
print(f"{len(tr)} languages")
