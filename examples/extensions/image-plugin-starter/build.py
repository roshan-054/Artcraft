#!/usr/bin/env python3
"""Build the independent Image Gain panel, native worker and packaged WASM filter."""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def command(arguments, cwd, offline=False, capture=False):
    arguments = [str(value) for value in arguments]
    if offline:
        arguments.append("--offline")
    result = subprocess.run(arguments, cwd=cwd, text=True, encoding="utf-8", capture_output=capture, check=True)
    return result.stdout if capture else None


def platform_name():
    systems = {"Windows": "windows", "Linux": "linux", "Darwin": "macos"}
    architectures = {"amd64": "x86_64", "x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}
    system, architecture = systems.get(platform.system()), architectures.get(platform.machine().lower())
    if system is None or architecture is None:
        raise ValueError("Only Windows/Linux/macOS x86-64 and ARM64 packages are supported.")
    return f"{system}-{architecture}"


def target_directory(project):
    configured = os.environ.get("CARGO_TARGET_DIR")
    directory = Path(configured) if configured else project / "target"
    return directory if directory.is_absolute() else (project / directory).resolve()


def dependency_licenses(projects, destination, offline):
    copied = set()
    for project in projects:
        metadata = json.loads(command(["cargo", "metadata", "--locked", "--format-version", "1"], project, offline, capture=True))
        for package in metadata["packages"]:
            if not package.get("source"):
                continue
            identity = f"{package['name']}-{package['version']}"
            if identity in copied:
                continue
            source = Path(package["manifest_path"]).parent
            files = sorted(path for path in source.iterdir() if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE")))
            if not files:
                raise ValueError(f"Missing license files for {identity}; review the dependency before packaging.")
            target = destination / identity
            target.mkdir(parents=True, exist_ok=True)
            for path in files:
                shutil.copyfile(path, target / path.name)
            copied.add(identity)
    return sorted(copied)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached crates/toolchains only")
    parser.add_argument("--check", action="store_true", help="Run formatting, native tests and Clippy before building")
    args = parser.parse_args()
    projects = [ROOT, ROOT / "native", ROOT / "filter"]
    if args.check:
        for project in projects:
            command(["cargo", "fmt", "--all", "--", "--check"], project)
            command(["cargo", "test", "--locked"], project, args.offline)
            # Cargo options must precede the -- separating Clippy's own options.
            command(["cargo", "clippy", "--locked", "--all-targets", *( ["--offline"] if args.offline else []), "--", "-D", "warnings"], project)
    for project in (ROOT, ROOT / "filter"):
        command(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown"], project, args.offline)
    command(["cargo", "build", "--locked", "--release"], ROOT / "native", args.offline)
    host_platform = platform_name()
    suffix = ".exe" if host_platform.startswith("windows-") else ""
    worker_name = f"image-gain-worker{suffix}"
    distribution = ROOT / "dist"
    stage = distribution / "package"
    # Delete only our fixed generated staging directory after checking its parent.
    if stage.resolve() != ROOT / "dist/package" or stage.is_symlink() or getattr(stage, "is_junction", lambda: False)():
        raise ValueError("Unsafe generated staging directory.")
    if stage.exists():
        shutil.rmtree(stage)
    (stage / "filters").mkdir(parents=True)
    shutil.copyfile(target_directory(ROOT) / "wasm32-unknown-unknown/release/photocraft_image_gain_panel.wasm", stage / "plugin.pcext")
    shutil.copyfile(target_directory(ROOT / "filter") / "wasm32-unknown-unknown/release/photocraft_filter_channel_gain.wasm", stage / "filters/channel-gain.wasm")
    shutil.copyfile(target_directory(ROOT / "native") / f"release/{worker_name}", stage / worker_name)
    for name in ("LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "THIRD-PARTY-NOTICES.md"):
        shutil.copyfile(ROOT / name, stage / name)
    metadata = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    metadata["native"]["platform"] = host_platform
    metadata["native"]["entry"] = worker_name
    (stage / "package.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    licenses = dependency_licenses(projects, stage / "licenses", args.offline)
    (stage / "licenses/DEPENDENCIES.txt").write_text("\n".join(licenses) + "\n", encoding="utf-8")
    helper = ROOT / "tools/package-plugin.py"
    command([sys.executable, helper, "--root", stage, "--output", distribution / f"Image-Gain-{host_platform}.pcplugin"], ROOT)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Build failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
