#!/usr/bin/env python3
"""Build a Sileo/apt repo (flat + dists/) from one or more .deb files."""
from __future__ import annotations

import argparse
import bz2
import gzip
import hashlib
import re
import shutil
import subprocess
import time
from email.utils import formatdate
from pathlib import Path


def sha(data: bytes, name: str) -> str:
    h = getattr(hashlib, name)()
    h.update(data)
    return h.hexdigest()


def control_fields(deb: Path) -> dict[str, str]:
    """Read control fields; prefer dpkg-deb, fall back to ar/tar parse."""
    try:
        out = subprocess.check_output(["dpkg-deb", "-f", str(deb)], text=True)
        fields: dict[str, str] = {}
        key = None
        for line in out.splitlines():
            if not line:
                continue
            if line[0].isspace() and key:
                fields[key] += "\n" + line
                continue
            if ":" in line:
                key, val = line.split(":", 1)
                fields[key.strip()] = val.strip()
        return fields
    except (FileNotFoundError, subprocess.CalledProcessError):
        pass

    # Minimal fallback from filename: name_ver_arch.deb
    m = re.match(r"^(?P<pkg>.+)_(?P<ver>[^_]+)_(?P<arch>.+)\.deb$", deb.name)
    if not m:
        raise SystemExit(f"cannot parse control for {deb}")
    return {
        "Package": m.group("pkg"),
        "Version": m.group("ver"),
        "Architecture": "iphoneos-arm64",
        "Maintainer": "ioscpy <ioscpy@local>",
        "Description": "Mirror and control this device over USB.",
        "Section": "Tweaks",
        "Depends": "ellekit | mobilesubstrate, firmware (>= 14.0)",
        "Name": "ioscpy",
    }


def packages_entry(deb_name: str, data: bytes, fields: dict[str, str]) -> str:
    # Stable field order; ASCII-only values so apt/Sileo never choke on quotes.
    pkg = fields.get("Package", "com.ioscpy.device")
    name = fields.get("Name", "ioscpy")
    ver = fields.get("Version", "0")
    desc = fields.get("Description", "ioscpy").split("\n", 1)[0].strip()
    depends = fields.get("Depends", "ellekit | mobilesubstrate, firmware (>= 14.0)")
    section = fields.get("Section", "Tweaks")
    # Force Dopamine/Sileo arch even if Theos stamped arm64e.
    return (
        f"Package: {pkg}\n"
        f"Name: {name}\n"
        f"Version: {ver}\n"
        f"Architecture: iphoneos-arm64\n"
        f"Maintainer: ioscpy <ioscpy@local>\n"
        f"Author: ioscpy <ioscpy@local>\n"
        f"Section: {section}\n"
        f"Depends: {depends}\n"
        f"Description: {desc}\n"
        f"Filename: ./{deb_name}\n"
        f"Size: {len(data)}\n"
        f"MD5sum: {sha(data, 'md5')}\n"
        f"SHA1: {sha(data, 'sha1')}\n"
        f"SHA256: {sha(data, 'sha256')}\n"
        "\n"
    )


def release_body(file_map: dict[str, bytes]) -> str:
    md5_lines, sha1_lines, sha256_lines = [], [], []
    for rel, data in sorted(file_map.items()):
        md5_lines.append(f" {sha(data, 'md5')} {len(data)} {rel}")
        sha1_lines.append(f" {sha(data, 'sha1')} {len(data)} {rel}")
        sha256_lines.append(f" {sha(data, 'sha256')} {len(data)} {rel}")
    return (
        "Origin: ioscpy\n"
        "Label: ioscpy\n"
        "Suite: stable\n"
        "Version: 1.0\n"
        "Codename: ios\n"
        "Date: " + formatdate(time.time(), localtime=False) + "\n"
        "Architectures: iphoneos-arm64\n"
        "Components: main\n"
        "Description: ioscpy apt source\n"
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
    for deb in args.debs:
        if not deb.is_file():
            raise SystemExit(f"missing deb: {deb}")
        data = deb.read_bytes()
        fields = control_fields(deb)
        write_bytes(repo / deb.name, data)
        if "com.ioscpy.device_" in deb.name:
            write_bytes(repo / "ioscpy.deb", data)
        write_bytes(repo / "dists/stable/main/binary-iphoneos-arm64" / deb.name, data)
        entries.append(packages_entry(deb.name, data, fields))

    packages = "".join(entries).encode("utf-8")
    packages_gz = gzip.compress(packages, mtime=0)
    packages_bz2 = bz2.compress(packages)

    for root in (repo, repo / "dists/stable/main/binary-iphoneos-arm64"):
        write_bytes(root / "Packages", packages)
        write_bytes(root / "Packages.gz", packages_gz)
        write_bytes(root / "Packages.bz2", packages_bz2)

    write_bytes(
        repo / "Release",
        release_body(
            {
                "Packages": packages,
                "Packages.gz": packages_gz,
                "Packages.bz2": packages_bz2,
            }
        ).encode(),
    )
    write_bytes(
        repo / "dists/stable/Release",
        release_body(
            {
                "main/binary-iphoneos-arm64/Packages": packages,
                "main/binary-iphoneos-arm64/Packages.gz": packages_gz,
                "main/binary-iphoneos-arm64/Packages.bz2": packages_bz2,
            }
        ).encode(),
    )

    write_bytes(
        repo / "index.html",
        (
            "<!doctype html><meta charset=utf-8><title>ioscpy</title>"
            "<h1>ioscpy</h1>"
            "<p>Sileo source:</p>"
            "<pre>https://harm0n1aa.github.io/</pre>\n"
        ).encode(),
    )
    print(f"wrote apt repo -> {repo}")


if __name__ == "__main__":
    main()
