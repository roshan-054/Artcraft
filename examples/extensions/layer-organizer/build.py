#!/usr/bin/env python3
"""Build this independent WASM guest and its deterministic panel-only .pcplugin."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent


def command(arguments, offline=False, capture=False):
    if offline:
        arguments = [*arguments, "--offline"]
    result = subprocess.run([str(argument) for argument in arguments], cwd=ROOT, check=True, text=True, encoding="utf-8", capture_output=capture)
    return result.stdout if capture else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        command(["cargo", "fmt", "--all", "--", "--check"])
        command(["cargo", "test", "--locked"], args.offline)
        command(["cargo", "clippy", "--locked", "--all-targets", *(["--offline"] if args.offline else []), "--", "-D", "warnings"])
    command(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown"], args.offline)
    metadata = json.loads(command(["cargo", "metadata", "--locked", "--format-version", "1"], args.offline, capture=True))
    identity = json.loads((ROOT / "manifest.json").read_text(encoding="utf-8"))
    local = next(package for package in metadata["packages"] if package["source"] is None)
    target = Path(metadata["target_directory"])
    module = local["targets"][0]["name"].replace("-", "_") + ".wasm"
    distribution = ROOT / "dist"
    distribution.mkdir(exist_ok=True)
    # A fresh directory prevents stale payload members without recursively deleting user paths.
    with tempfile.TemporaryDirectory(prefix="package-", dir=distribution) as temporary:
        stage = Path(temporary)
        shutil.copyfile(target / "wasm32-unknown-unknown/release" / module, stage / "plugin.pcext")
        for name in ("package.json", "LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "THIRD-PARTY-NOTICES.md"):
            shutil.copyfile(ROOT / name, stage / name)
        dependencies = []
        for package in metadata["packages"]:
            if package["source"] is None:
                continue
            dependency = f"{package['name']}-{package['version']}"
            source = Path(package["manifest_path"]).parent
            licenses = sorted(path for path in source.iterdir() if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE")))
            if not licenses:
                raise ValueError(f"Review missing dependency licenses for {dependency} before packaging.")
            destination = stage / "licenses" / dependency
            destination.mkdir(parents=True)
            for license_path in licenses:
                shutil.copyfile(license_path, destination / license_path.name)
            dependencies.append(dependency)
        (stage / "licenses/DEPENDENCIES.txt").write_text("\n".join(sorted(dependencies)) + "\n", encoding="utf-8")
        output = distribution / f"{identity['name'].replace(' ', '-')}-{identity['version']}.pcplugin"
        command([sys.executable, ROOT / "tools/package-plugin.py", "--root", stage, "--output", output])
        print(output)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Build failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
