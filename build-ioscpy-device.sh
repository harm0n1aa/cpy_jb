#!/usr/bin/env bash
# Build the patched ioscpy .deb inside WSL/Ubuntu (Theos).
# Usage:
#   wsl -e bash /mnt/s/programms/cpy_jb/build-ioscpy-device.sh
#   wsl -e bash /mnt/s/programms/cpy_jb/build-ioscpy-device.sh rootful
set -euo pipefail

SCHEME="${1:-rootless}"
ROOT="$(cd "$(dirname "$0")" && pwd)"
export THEOS="${THEOS:-$HOME/theos}"

if [[ ! -x "$THEOS/bin/make" && ! -d "$THEOS" ]]; then
  echo ">> installing Theos into $THEOS"
  sudo apt-get update
  sudo apt-get install -y bash curl sudo git perl unzip build-essential fakeroot
  if grep -qi microsoft /proc/version 2>/dev/null; then
    sudo update-alternatives --set fakeroot /usr/bin/fakeroot-tcp || true
  fi
  bash -c "$(curl -fsSL https://raw.githubusercontent.com/theos/theos/master/bin/install-theos)"
fi

if [[ ! -d "$THEOS/sdks" ]] || ! compgen -G "$THEOS/sdks/*.sdk" >/dev/null; then
  echo ">> fetching iOS SDKs"
  mkdir -p "$THEOS/sdks"
  tmp="$(mktemp -d)"
  curl -L "https://github.com/theos/sdks/archive/master.zip" -o "$tmp/sdks.zip"
  unzip -q "$tmp/sdks.zip" -d "$tmp"
  mv "$tmp"/sdks-master/*.sdk "$THEOS/sdks/" || true
  rm -rf "$tmp"
fi

cd "$ROOT/ioscpy/device"
make clean || true
if [[ "$SCHEME" == "rootful" ]]; then
  echo ">> building rootful package"
  make package FINALPACKAGE=1
else
  echo ">> building rootless package (Dopamine / palera1n-rootless)"
  make package THEOS_PACKAGE_SCHEME=rootless FINALPACKAGE=1
fi

DEB="$(ls -t "$ROOT"/ioscpy/device/packages/*.deb | head -1)"
echo
echo "Built: $DEB"
echo
echo "Put this .deb on the iPhone and install it:"
echo "  Filza -> Open -> Install, then Respring"
echo "Or with OpenSSH from Windows PowerShell:"
echo "  iproxy 2222 22"
echo "  scp -P 2222 \"$DEB\" root@127.0.0.1:/tmp/ioscpy.deb"
echo "  ssh -p 2222 root@127.0.0.1 \"dpkg -i /tmp/ioscpy.deb && sbreload\""
