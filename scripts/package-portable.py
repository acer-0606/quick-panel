#!/usr/bin/env python3
"""Create a fresh portable package; verify every member's bytes, CRC and SHA256."""
import argparse
import hashlib
import json
import pathlib
import shutil
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
NODE_SHA = "3602f2bb1a10f2cbab4c36886218a33c1ab3db87290e73b033c46c77147d0237"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--target", default="x86_64-pc-windows-gnu")
parser.add_argument("--node-assets", type=pathlib.Path, required=True)
args = parser.parse_args()
assets = args.node_assets.resolve()
if hashlib.sha256((assets / "node.exe").read_bytes()).hexdigest() != NODE_SHA:
    raise SystemExit("Node runtime is not the pinned, verified 24.19.0 Windows x64 executable")
exe = ROOT / "target" / args.target / "release/quick-panel.exe"
module = ROOT / "agent-runtime/dist/worker.mjs"
if not exe.is_file() or not module.is_file():
    raise SystemExit("Build the release executable and Pi bundle before packaging")
dist = ROOT / "dist"
dist.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix=".qp-package-", dir=dist) as temporary:
    stage = pathlib.Path(temporary) / "QuickPanel-Windows-x64"
    (stage / "runtime").mkdir(parents=True)
    mapping = {"QuickPanel.exe": exe, "runtime/node.exe": assets / "node.exe",
               "runtime/pi-harness.mjs": module, "runtime/NODE-LICENSE.txt": assets / "LICENSE",
               "runtime/PI-LICENSES.txt": ROOT / "agent-runtime/THIRD_PARTY_NOTICES.txt"}
    for name in ["README.md", "LICENSE", "THIRD_PARTY_NOTICES.txt"]:
        mapping[name] = ROOT / name
    for file in (ROOT / "docs").rglob("*"):
        if file.is_file() and file.suffix in {".md", ".txt"}:
            mapping[file.relative_to(ROOT).as_posix()] = file
    for name, source in mapping.items():
        destination = stage / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    archive = pathlib.Path(temporary) / "QuickPanel-Windows-x64.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for name in sorted(mapping):
            z.write(stage / name, name)
    members = []
    with zipfile.ZipFile(archive) as z:
        assert len(z.namelist()) == len(mapping) and set(z.namelist()) == set(mapping), "Unexpected or duplicate ZIP member"
        assert z.testzip() is None, "ZIP CRC mismatch"
        for name in sorted(mapping):
            expected = (stage / name).read_bytes()
            actual = z.read(name)
            assert actual == expected, f"Member bytes mismatch: {name}"
            members.append({"path": name, "bytes": len(actual), "sha256": hashlib.sha256(actual).hexdigest()})
    final = dist / "QuickPanel-Windows-x64"
    if final.exists():
        shutil.rmtree(final)
    shutil.move(str(stage), final)
    final_zip = dist / archive.name
    archive.replace(final_zip)
    zipped = final_zip.read_bytes()
    manifest = {"archive": final_zip.name, "bytes": len(zipped), "sha256": hashlib.sha256(zipped).hexdigest(), "members": members}
    (dist / "package-manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    sums = [f"{manifest['sha256']}  {final_zip.name}"] + [f"{m['sha256']}  QuickPanel-Windows-x64/{m['path']}" for m in members]
    (dist / "SHA256SUMS").write_text("\n".join(sums) + "\n")
    print(json.dumps({key: value for key, value in manifest.items() if key != "members"}, ensure_ascii=False))
    print(f"Verified {len(members)} exact members, CRC and bytes; unpacked {sum(m['bytes'] for m in members)} bytes")
