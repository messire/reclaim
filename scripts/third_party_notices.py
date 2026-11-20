#!/usr/bin/env python3
"""Generates THIRD_PARTY_NOTICES.txt for the shipped binaries (PLAN M6.3).

Walks the normal (non-dev, non-build) dependency graph of the shipped packages for the
Windows target via `cargo metadata`, and writes each crate's name, version, license
expression and repository, followed by the license and notice files from its source.

Usage: python scripts/third_party_notices.py [--out THIRD_PARTY_NOTICES.txt]
"""

import json
import os
import subprocess
import sys

SHIPPED = ["reclaim-app", "reclaim-cli"]
TARGET = "x86_64-pc-windows-msvc"
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright")
SLINT_LICENSE = (
    "https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md"
)
MIT_TEXT = """MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE."""


def metadata():
    out = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET]
    )
    return json.loads(out)


def shipped_closure(meta):
    # `cargo tree -p` resolves features for the shipped packages alone; `cargo metadata`
    # would unify features across the whole workspace (spikes, benches) and over-report.
    tree = subprocess.check_output(
        ["cargo", "tree", "--locked", "--target", TARGET, "-e", "normal", "--prefix", "none",
         "-f", "{p}"] + [a for p in SHIPPED for a in ("-p", p)],
        text=True,
    )
    linked = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            linked.add((parts[0], parts[1][1:]))
    return sorted(
        (
            p
            for p in meta["packages"]
            if (p["name"], p["version"]) in linked
            and p["source"] is not None
            # Proc macros run at build time and are not distributed.
            and not any("proc-macro" in t["kind"] for t in p["targets"])
        ),
        key=lambda p: (p["name"], p["version"]),
    )


def license_texts(pkg):
    root = os.path.dirname(pkg["manifest_path"])
    texts = []
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name)
        if os.path.isfile(path) and name.lower().startswith(LICENSE_PREFIXES):
            with open(path, encoding="utf-8", errors="replace") as f:
                texts.append((name, f.read().strip()))
    if pkg.get("license_file"):
        path = os.path.join(root, pkg["license_file"])
        if os.path.isfile(path) and all(n != os.path.basename(path) for n, _ in texts):
            with open(path, encoding="utf-8", errors="replace") as f:
                texts.append((os.path.basename(path), f.read().strip()))
    return texts


def main():
    out_path = "THIRD_PARTY_NOTICES.txt"
    if "--out" in sys.argv:
        out_path = sys.argv[sys.argv.index("--out") + 1]
    pkgs = shipped_closure(metadata())
    lines = [
        "Third-party software in Reclaim",
        "================================",
        "",
        "Reclaim is MIT-licensed. It includes the following crates, under their own licenses.",
        "The user interface is built with Slint (https://slint.dev), used under the Slint",
        "Royalty-free Desktop, Mobile, and Web Applications License 2.0.",
        "",
        "Summary",
        "-------",
    ]
    for p in pkgs:
        lines.append(f"{p['name']} {p['version']}: {p.get('license') or 'see license file'}")
    missing = []
    for p in pkgs:
        lines += ["", "=" * 78, f"{p['name']} {p['version']}", f"License: {p.get('license') or '-'}"]
        if p.get("repository"):
            lines.append(f"Source: {p['repository']}")
        texts = license_texts(p)
        if not texts:
            missing.append(p["name"])
            if "Slint" in (p.get("license") or ""):
                lines.append("(license text: " + SLINT_LICENSE + ")")
            else:
                lines.append("(no license file in the published crate; the license expression above"
                             " applies, standard texts at the end of this file)")
        for name, text in texts:
            lines += ["", f"--- {name} ---", text]
    lines += ["", "=" * 78, "Standard license texts", "", MIT_TEXT, "",
              "Apache License 2.0: https://www.apache.org/licenses/LICENSE-2.0.txt"]
    with open(out_path, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(f"{out_path}: {len(pkgs)} crates, {len(missing)} without a license file")
    if missing:
        print("  without license file: " + ", ".join(missing))


if __name__ == "__main__":
    main()
