#!/usr/bin/env python3
"""Build a flat Sileo/apt repo directory from one or more .deb files."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import re
import subprocess
from pathlib import Path


def sha(data: bytes, name: str) -> str:
    h = getattr(hashlib, name)()
    h.update(data)
    return h.hexdigest()


def control_from_deb(deb: Path) -> str:
    out = subprocess.check_output(["dpkg-deb", "-f", str(deb)], text=True)
    # Drop fields that we rewrite for the repo index.
    drop = {"Filename", "Size", "MD5sum", "SHA1", "SHA256", "Status"}
    lines = []
    for line in out.splitlines():
        key = line.split(":", 1)[0].strip()
        if key in drop:
            continue
        lines.append(line.rstrip())
    while lines and not lines[-1].strip():
        lines.pop()
    return "\n".join(lines) + "\n"


def packages_entry(deb: Path) -> str:
    data = deb.read_bytes()
    ctrl = control_from_deb(deb)
    return (
        f"{ctrl}"
        f"Filename: {deb.name}\n"
        f"Size: {len(data)}\n"
        f"MD5sum: {sha(data, 'md5')}\n"
        f"SHA1: {sha(data, 'sha1')}\n"
        f"SHA256: {sha(data, 'sha256')}\n"
        "\n"
    )


def write_release(repo: Path, packages: bytes, packages_gz: bytes) -> None:
    # Minimal unsigned Release; Sileo accepts it for third-party sources.
    body = (
        "Origin: ioscpy\n"
        "Label: ioscpy\n"
        "Suite: stable\n"
        "Version: 1.0\n"
        "Codename: ios\n"
        "Architectures: iphoneos-arm64\n"
        "Components: main\n"
        "Description: ioscpy Mac-built packages (arm64 + arm64e)\n"
        f"MD5Sum:\n {sha(packages, 'md5')} {len(packages)} Packages\n"
        f" {sha(packages_gz, 'md5')} {len(packages_gz)} Packages.gz\n"
        f"SHA256:\n {sha(packages, 'sha256')} {len(packages)} Packages\n"
        f" {sha(packages_gz, 'sha256')} {len(packages_gz)} Packages.gz\n"
    )
    (repo / "Release").write_text(body, encoding="utf-8")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("debs", nargs="+", type=Path)
    args = ap.parse_args()

    repo = args.out
    repo.mkdir(parents=True, exist_ok=True)
    entries: list[str] = []
    for deb in args.debs:
        if not deb.is_file():
            raise SystemExit(f"missing deb: {deb}")
        dest = repo / deb.name
        dest.write_bytes(deb.read_bytes())
        # Stable alias for humans / docs.
        if re.search(r"com\.ioscpy\.device_", deb.name):
            (repo / "ioscpy.deb").write_bytes(deb.read_bytes())
        entries.append(packages_entry(dest))

    packages = "".join(entries).encode("utf-8")
    packages_gz = gzip.compress(packages, mtime=0)
    (repo / "Packages").write_bytes(packages)
    (repo / "Packages.gz").write_bytes(packages_gz)
    write_release(repo, packages, packages_gz)
    (repo / "index.html").write_text(
        "<!doctype html><meta charset=utf-8><title>ioscpy</title>"
        "<h1>ioscpy apt source</h1>"
        "<p>Add this URL in Sileo → Sources:</p>"
        "<pre>https://harm0n1aa.github.io/cpy_jb/</pre>"
        "<p>Then search <b>ioscpy</b> and install.</p>\n",
        encoding="utf-8",
    )
    print(f"wrote apt repo -> {repo} ({len(args.debs)} deb(s))")


if __name__ == "__main__":
    main()
