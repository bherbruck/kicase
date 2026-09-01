#!/usr/bin/env python3
"""Builds the KiCad Plugin and Content Manager package for one platform.

KiCase ships a compiled binary, so a package is per-platform. PCM handles that
through the `platforms` field on each version entry: one repository carries all
three builds and KiCad downloads the one that runs on the machine asking.

    python3 packaging/build_package.py --platform linux --out dist/

Produces dist/kicase-<version>-<platform>.zip laid out the way PCM requires:

    metadata.json
    plugins/          the binary and the manifest KiCad's API loads
    resources/icon.png
"""

import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
# The binary's name differs on Windows, and so does the manifest that names it.
BINARY = {"linux": "kicase", "macos": "kicase", "windows": "kicase.exe"}
MANIFEST = {
    "linux": "plugin.json",
    "macos": "plugin.json",
    "windows": "plugin.windows.json",
}


def version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    for line in text.splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit("no version in Cargo.toml")


def build(platform: str, out: pathlib.Path) -> pathlib.Path:
    binary = ROOT / "bin" / "release" / BINARY[platform]
    if not binary.exists():
        raise SystemExit(f"no binary at {binary}; run cargo build --release -p kicase-app first")

    staging = out / f"stage-{platform}"
    shutil.rmtree(staging, ignore_errors=True)
    (staging / "plugins" / "icons").mkdir(parents=True)
    (staging / "resources").mkdir(parents=True)

    # KiCad loads plugin.json from the package's plugins/ directory, so the
    # manifest is always called that whichever platform it came from.
    shutil.copy(ROOT / "plugin" / MANIFEST[platform], staging / "plugins" / "plugin.json")
    shutil.copy(binary, staging / "plugins" / BINARY[platform])
    (staging / "plugins" / BINARY[platform]).chmod(0o755)
    for icon in (ROOT / "plugin" / "icons").glob("*.png"):
        shutil.copy(icon, staging / "plugins" / "icons" / icon.name)
    shutil.copy(ROOT / "plugin" / "icons" / "kicase-64.png", staging / "resources" / "icon.png")

    metadata = json.loads((ROOT / "packaging" / "metadata.json").read_text())
    # The archive's own copy carries no download_url: that is the repository's
    # job, and PCM rejects a version entry that claims both.
    metadata["versions"] = [
        {
            "version": version(),
            "status": "stable",
            "kicad_version": "9.0",
            "platforms": [platform],
            "runtime": "ipc",
        }
    ]
    (staging / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

    archive = out / f"kicase-{version()}-{platform}.zip"
    archive.unlink(missing_ok=True)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
        for path in sorted(staging.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(str(path.relative_to(staging)))
                # Executable bit, or the plugin will not run once unpacked.
                info.external_attr = (0o755 if path.name.startswith("kicase") else 0o644) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                zf.writestr(info, path.read_bytes())
    shutil.rmtree(staging)
    return archive


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=sorted(BINARY))
    parser.add_argument("--out", default="dist", type=pathlib.Path)
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    archive = build(args.platform, args.out)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    print(f"{archive}  {archive.stat().st_size} bytes  sha256 {digest}")


if __name__ == "__main__":
    sys.exit(main())
