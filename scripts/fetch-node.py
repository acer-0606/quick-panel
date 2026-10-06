#!/usr/bin/env python3
"""Explicit developer-only download of the pinned Windows runtime."""
import argparse
import hashlib
import pathlib
import urllib.request
import zipfile

VERSION = "24.19.0"
ZIP_SHA = "57f71ab3652e797d84acddc79c81cc9ff1c6ddb2a1974cdb83f00fee9bff4c73"
NODE_SHA = "3602f2bb1a10f2cbab4c36886218a33c1ab3db87290e73b033c46c77147d0237"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--download", action="store_true", required=True)
parser.add_argument("--output", type=pathlib.Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
name = f"node-v{VERSION}-win-x64"
url = f"https://nodejs.org/dist/v{VERSION}/{name}.zip"
archive = args.output / f"{name}.zip"
with urllib.request.urlopen(url, timeout=60) as response:
    data = response.read(64 * 1024 * 1024 + 1)
if hashlib.sha256(data).hexdigest() != ZIP_SHA:
    raise SystemExit("Pinned official Node ZIP SHA256 mismatch")
archive.write_bytes(data)
with zipfile.ZipFile(archive) as z:
    if z.testzip() is not None:
        raise SystemExit("Node ZIP CRC mismatch")
    exe = z.read(f"{name}/node.exe")
    if hashlib.sha256(exe).hexdigest() != NODE_SHA:
        raise SystemExit("Pinned Node executable SHA256 mismatch")
    (args.output / "node.exe").write_bytes(exe)
    (args.output / "LICENSE").write_bytes(z.read(f"{name}/LICENSE"))
print(f"Verified official Node {VERSION} Windows x64: {NODE_SHA}")
