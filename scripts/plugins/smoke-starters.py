"""Exercise both neutral release modules through the actual PhotoCraft CLI host."""

import argparse
import json
from pathlib import Path
import subprocess


def main():
    if not __debug__:
        raise RuntimeError("Run this test script without Python -O; its checks require assertions.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--panel", type=Path, required=True)
    parser.add_argument("--filter", type=Path, required=True)
    parser.add_argument("--logs", type=Path)
    args = parser.parse_args()
    cli, panel, pixel_filter = (path.resolve() for path in (args.cli, args.panel, args.filter))
    if args.logs:
        args.logs.mkdir(parents=True, exist_ok=True)

    def run(label, commands, settings=None, error=None):
        command = [str(cli), "run", "--new", json.dumps(settings or {"width": 8, "height": 6, "background": "transparent"})]
        for name, params in commands:
            command += ["--cmd", name, "--params", json.dumps(params)]
        result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=120)
        if args.logs:
            (args.logs / f"developer-kit-{label}.log").write_text(result.stdout + "\n" + result.stderr, encoding="utf-8")
        if error is None:
            if result.returncode:
                raise RuntimeError(f"{label}: {result.stderr}")
        elif not result.returncode or error.lower() not in result.stderr.lower():
            raise AssertionError(f"{label}: expected error containing {error!r}, got {result.returncode}: {result.stderr}")
        rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        print(f"{label}: command execution checked")
        return rows

    install_panel = ("extension.install", {"path": str(panel)})
    create = ("extension.invoke", {"id": "org.example.hello-photocraft", "event": "create", "values": {"layerName": "Notes"}})
    info = ("extension.invoke", {"id": "org.example.hello-photocraft", "event": "info", "values": {}})
    inspect = ("document.inspect", {})
    rows = run("panel", [install_panel, info, inspect, create, inspect, ("edit.undo", {}), inspect, ("edit.redo", {}), inspect])
    snapshots = [row["result"] for row in rows if row["command"] == "document.inspect"]
    before, applied, undone, redone = snapshots
    assert len(applied["layers"]) == len(before["layers"]) + 1
    assert applied["layers"][0]["name"] == "Notes"
    assert len(undone["layers"]) == len(before["layers"])
    assert len(redone["layers"]) == len(applied["layers"])
    assert rows[1]["result"]["results"] == []
    no_document = run("panel-no-document", [install_panel, ("file.close", {}), info])
    assert no_document[-1]["result"]["results"] == []
    assert "open" in no_document[-1]["result"]["message"].lower()
    run("panel-invalid-choice", [install_panel, ("extension.invoke", {"id": "org.example.hello-photocraft", "event": "create", "values": {"layerName": "Not an option"}})], error="must be one of")
    run("panel-disabled", [install_panel, ("extension.enable", {"id": "org.example.hello-photocraft", "enabled": False}), create], error="disabled")
    run("panel-removed", [install_panel, ("extension.remove", {"id": "org.example.hello-photocraft"}), create], error="not installed")
    replace = run("panel-replace", [install_panel, ("extension.install", {"path": str(panel), "replace": True}), create, inspect])
    assert replace[1]["result"]["replaced"] is True
    assert replace[-1]["result"]["layers"][0]["name"] == "Notes"

    install_filter = ("plugin.install", {"path": str(pixel_filter)})
    fill = ("edit.fill", {"color": [0.2, 0.4, 0.6, 0.5]})
    pixel = ("document.pixel", {"x": 2, "y": 2})
    apply_filter = ("plugin.run", {"id": "org.example.channel-gain", "params": {"gain": 1.5}})
    for depth in (8, 16, 32):
        rows = run(f"filter-{depth}", [fill, pixel, install_filter, apply_filter, pixel, ("edit.undo", {}), pixel, ("edit.redo", {}), pixel],
                   {"width": 8, "height": 6, "depth": depth, "background": "transparent"})
        before, after, undone, redone = [row["result"] for row in rows if row["command"] == "document.pixel"]
        tolerance = {8: 1 / 255 + 1e-6, 16: 1 / 65535 + 1e-6, 32: 1e-6}[depth]
        assert all(abs(after[i] - before[i] * 1.5) <= tolerance for i in range(3))
        assert after[3] == before[3], "filter alpha changed"
        assert undone == before, "Undo did not restore the source"
        assert redone == after, "Redo did not restore the result"
    outside = ("document.pixel", {"x": 6, "y": 2})
    rows = run("filter-selection", [fill, pixel, outside, install_filter,
               ("select.rect", {"x": 0, "y": 0, "width": 4, "height": 6}), apply_filter, pixel, outside])
    inside_before, outside_before, inside_after, outside_after = [row["result"] for row in rows if row["command"] == "document.pixel"]
    assert inside_after != inside_before
    assert outside_after == outside_before, "filter changed unselected pixels"
    run("filter-invalid-gain", [fill, install_filter, ("plugin.run", {"id": "org.example.channel-gain", "params": {"gain": "wrong type"}})], error="number")
    run("filter-unsupported-mode", [install_filter, apply_filter], {"width": 8, "height": 6, "mode": "cmyk"}, error="pc_filter returned error code 3")
    print("All 12 compiled-module host cases passed. No persistent plugin store or user document was used.")


if __name__ == "__main__":
    main()
