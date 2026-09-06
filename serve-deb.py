"""Serve a tiny Sileo/apt repo on LAN so the phone can install ioscpy without Filza."""
from __future__ import annotations

import gzip
import hashlib
import http.server
import os
import socket
import ssl
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CANDIDATES = tuple(
    sorted(
        list(ROOT.glob("com.ioscpy.device_*.deb"))
        + list((ROOT / "ioscpy" / "device" / "packages").glob("com.ioscpy.device_*.deb"))
        + [ROOT / "ioscpy-sileo.deb"],
        key=lambda p: p.stat().st_mtime if p.is_file() else 0,
        reverse=True,
    )
)
PORT = 8080
TLS_PORT = 8080
HTTP_PORT = 8081


def deb_path() -> Path:
    for p in CANDIDATES:
        if p.is_file():
            return p
    raise SystemExit("ioscpy .deb not found")


def hashes(data: bytes) -> dict[str, str]:
    return {
        "md5": hashlib.md5(data).hexdigest(),
        "sha1": hashlib.sha1(data).hexdigest(),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def packages_text(deb: Path, data: bytes) -> str:
    h = hashes(data)
    name = deb.name
    version = "0.1.14"
    parts = name.split("_")
    if len(parts) >= 2:
        version = parts[1]
    return f"""Package: com.ioscpy.device
Name: ioscpy
Version: {version}
Architecture: iphoneos-arm64
Maintainer: Lautaro Villarreal Culic' <lautaro@lautarovculic.com>
Depends: ellekit | mobilesubstrate, firmware (>= 14.0)
Filename: {name}
Size: {len(data)}
MD5sum: {h['md5']}
SHA1: {h['sha1']}
SHA256: {h['sha256']}
Section: Tweaks
Description: Mirror and control this device over USB.

"""


def release_text() -> str:
    return """Origin: ioscpy-local
Label: ioscpy-local
Suite: stable
Version: 1.0
Codename: ios
Architectures: iphoneos-arm64
Components: main
Description: Temporary LAN repo to install ioscpy
"""


def ensure_tls_files() -> tuple[Path, Path]:
    key = ROOT / "repo-key.pem"
    crt = ROOT / "repo-cert.pem"
    if key.is_file() and crt.is_file():
        return key, crt
    subprocess.check_call(
        [sys.executable, "-m", "pip", "install", "--user", "cryptography", "-q"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    from datetime import datetime, timedelta, timezone

    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID

    pkey = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    names = [x509.DNSName("localhost"), x509.DNSName("ioscpy.local")]
    for ip in lan_ips():
        try:
            names.append(x509.IPAddress(__import__("ipaddress").ip_address(ip)))
        except ValueError:
            pass
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "ioscpy-local")])
    now = datetime.now(timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(pkey.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now)
        .not_valid_after(now + timedelta(days=3650))
        .add_extension(x509.SubjectAlternativeName(names), critical=False)
        .sign(pkey, hashes.SHA256())
    )
    key.write_bytes(
        pkey.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.TraditionalOpenSSL,
            serialization.NoEncryption(),
        )
    )
    crt.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    return key, crt


def lan_ips() -> list[str]:
    ips: list[str] = []
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
            ip = info[4][0]
            if ip.startswith("127.") or ip in ips:
                continue
            ips.append(ip)
    except OSError:
        pass
    return ips


def serve(httpd: http.server.HTTPServer) -> None:
    httpd.serve_forever()


class RepoHandler(http.server.BaseHTTPRequestHandler):
    files: dict[str, bytes] = {}

    def log_message(self, fmt: str, *args) -> None:
        print(fmt % args)

    def do_HEAD(self) -> None:
        path = self.path.split("?", 1)[0]
        if path == "/":
            body = b"ioscpy repo. Add this URL as a source in Sileo.\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            return
        key = path.lstrip("/")
        data = self.files.get(key) or self.files.get(key.removeprefix("./"))
        if data is None:
            self.send_error(404)
            return
        ctype = "application/vnd.debian.binary-package" if key.endswith(".deb") else "text/plain"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        if path == "/":
            body = b"ioscpy repo. Add this URL as a source in Sileo.\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        key = path.lstrip("/")
        data = self.files.get(key) or self.files.get(key.removeprefix("./"))
        if data is None:
            self.send_error(404)
            return
        ctype = "application/vnd.debian.binary-package" if key.endswith(".deb") else "text/plain"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main() -> None:
    deb = deb_path()
    data = deb.read_bytes()
    pkgs = packages_text(deb, data).encode("utf-8")
    rel = release_text().encode("utf-8")
    name = deb.name
    RepoHandler.files = {
        name: data,
        "ioscpy.deb": data,
        "Packages": pkgs,
        "Packages.gz": gzip.compress(pkgs, mtime=0),
        "Release": rel,
        "dists/stable/Release": rel,
        "dists/stable/main/binary-iphoneos-arm64/Packages": pkgs,
        "dists/stable/main/binary-iphoneos-arm64/Packages.gz": gzip.compress(pkgs, mtime=0),
        "dists/stable/main/binary-iphoneos-arm64/" + name: data,
    }
    print(f"repo {deb} ({len(data)} bytes)")
    print()
    print("  Sileo adds HTTPS by itself. Use 192.168, not 26.x:")
    print()
    for ip in lan_ips() or ["192.168.1.8"]:
        print(f"    https://{ip}:{TLS_PORT}")
        print(f"    http://{ip}:{HTTP_PORT}")
    print()
    print("  If Sileo warns about certificate, add anyway.")
    print(f"  Refresh, search ioscpy, install {name}")
    print()

    http_srv = http.server.HTTPServer(("0.0.0.0", HTTP_PORT), RepoHandler)
    threading.Thread(target=serve, args=(http_srv,), daemon=True).start()

    key, crt = ensure_tls_files()
    tls_srv = http.server.HTTPServer(("0.0.0.0", TLS_PORT), RepoHandler)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(str(crt), str(key))
    tls_srv.socket = ctx.wrap_socket(tls_srv.socket, server_side=True)
    tls_srv.serve_forever()


if __name__ == "__main__":
    os.chdir(ROOT)
    main()
