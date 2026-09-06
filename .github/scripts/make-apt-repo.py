#!/usr/bin/env python3
"""Build a Sileo/apt repo (flat + dists/) from one or more .deb files."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import re
import shutil
import subprocess
from pathlib import Path


def sha(data: bytes, name: str) -> str:
    h = getattr(hashlib, name)()
    h.update(data)
    return h.hexdigest()


def control_from_deb(deb: Path) -> str:
    out = subprocess.check_output(["dpkg-deb", "-f", str(deb)], text=True)
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


def packages_entry(deb_name: str, data: bytes, ctrl: str) -> str:
    return (
        f"{ctrl}"
        f"Filename: {deb_name}\n"
        f"Size: {len(data)}\n"
        f"MD5sum: {sha(data, 'md5')}\n"
        f"SHA1: {sha(data, 'sha1')}\n"
        f"SHA256: {sha(data, 'sha256')}\n"
        "\n"
    )


def release_body(file_map: dict[str, bytes]) -> str:
    """file_map: path relative to the Release file's directory -> content."""
    md5_lines = []
    sha1_lines = []
    sha256_lines = []
    for rel, data in sorted(file_map.items()):
        md5_lines.append(f" {sha(data, 'md5')} {len(data)} {rel}")
        sha1_lines.append(f" {sha(data, 'sha1')} {len(data)} {rel}")
        sha256_lines.append(f" {sha(data, 'sha256')} {len(data)} {rel}")
    return (
        "Origin: ioscpy\n"
        "Label: ioscpy\n"
        "Suite: stable\n"
        "Version: 1.0\n"
        "Codename: stable\n"
        "Architectures: iphoneos-arm64\n"
        "Components: main\n"
        "Description: ioscpy Mac-built packages (arm64 + arm64e)\n"
        "MD5Sum:\n"
        + "\n".join(md5_lines)
        + "\nSHA1:\n"
        + "\n".join(sha1_lines)
        + "\nSHA256:\n"
        + "\n".join(sha256_lines)
        + "\n"
    )


def write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("debs", nargs="+", type=Path)
    args = ap.parse_args()

    repo = args.out
    if repo.exists():
        shutil.rmtree(repo)
    repo.mkdir(parents=True)

    entries: list[str] = []
    deb_files: list[tuple[str, bytes]] = []
    for deb in args.debs:
        if not deb.is_file():
            raise SystemExit(f"missing deb: {deb}")
        data = deb.read_bytes()
        ctrl = control_from_deb(deb)
        write_bytes(repo / deb.name, data)
        if re.search(r"com\.ioscpy\.device_", deb.name):
            write_bytes(repo / "ioscpy.deb", data)
        # Also under dists path (some clients resolve relative to binary-*).
        write_bytes(
            repo / "dists/stable/main/binary-iphoneos-arm64" / deb.name,
            data,
        )
        deb_files.append((deb.name, data))
        entries.append(packages_entry(deb.name, data, ctrl))

    packages = "".join(entries).encode("utf-8")
    packages_gz = gzip.compress(packages, mtime=0)

    # Flat root (Cydia-style) — same as our LAN serve-deb.py.
    write_bytes(repo / "Packages", packages)
    write_bytes(repo / "Packages.gz", packages_gz)
    write_bytes(
        repo / "Release",
        release_body({"Packages": packages, "Packages.gz": packages_gz}).encode(),
    )

    # Standard apt dists tree — what modern Sileo/APT actually fetches.
    bin_dir = "main/binary-iphoneos-arm64"
    write_bytes(repo / "dists/stable" / bin_dir / "Packages", packages)
    write_bytes(repo / "dists/stable" / bin_dir / "Packages.gz", packages_gz)
    write_bytes(
        repo / "dists/stable/Release",
        release_body(
            {
                f"{bin_dir}/Packages": packages,
                f"{bin_dir}/Packages.gz": packages_gz,
            }
        ).encode(),
    )

    write_bytes(
        repo / "index.html",
        (
            "<!doctype html><meta charset=utf-8><title>ioscpy</title>"
            "<h1>ioscpy apt source</h1>"
            "<p>Add this URL in Sileo → Sources:</p>"
            "<pre>https://harm0n1aa.github.io/cpy_jb/</pre>"
            "<p>Then refresh sources, search <b>ioscpy</b>, install 0.1.27+.</p>\n"
        ).encode(),
    )
    print(f"wrote apt repo -> {repo} ({len(args.debs)} deb(s))")
    for name, _ in deb_files:
        print(f"  {name}")


if __name__ == "__main__":
    main()
