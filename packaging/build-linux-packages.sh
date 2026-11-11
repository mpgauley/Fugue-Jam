#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
binary="$root/target/release/fugue-jam"
output="$root/target/packages"
stage="$root/target/package-stage"
arch_stage="$root/target/arch-package-stage"

if [[ ! -x "$binary" ]]; then
  echo "Release binary not found: $binary" >&2
  exit 1
fi

mkdir -p "$output" "$stage/usr/bin" "$stage/usr/share/applications" \
  "$stage/usr/share/doc/fugue-jam" "$arch_stage/usr/bin" \
  "$arch_stage/usr/share/applications" "$arch_stage/usr/share/doc/fugue-jam"
install -m 755 "$binary" "$stage/usr/bin/fugue-jam"
install -m 644 packaging/fugue-jam.desktop "$stage/usr/share/applications/fugue-jam.desktop"
install -m 644 README.md LICENSE "$stage/usr/share/doc/fugue-jam/"

cargo deb --locked
cp target/debian/*.deb "$output/"

fpm \
  --input-type dir \
  --output-type rpm \
  --name fugue-jam \
  --version "$version" \
  --architecture x86_64 \
  --license Apache-2.0 \
  --description "A local-first Linux music player with a persistent library, playlists, queue, and equalizer." \
  --url https://github.com/mpgauley/Fugue-Jam \
  --depends 'libasound.so.2()(64bit)' \
  --depends fontconfig \
  --chdir "$stage" \
  --package "$output/fugue-jam-$version-1.x86_64.rpm" \
  usr/bin/fugue-jam \
  usr/share/applications/fugue-jam.desktop \
  usr/share/doc/fugue-jam/README.md \
  usr/share/doc/fugue-jam/LICENSE

install -m 755 "$binary" "$arch_stage/usr/bin/fugue-jam"
install -m 644 packaging/fugue-jam.desktop "$arch_stage/usr/share/applications/fugue-jam.desktop"
install -m 644 README.md LICENSE "$arch_stage/usr/share/doc/fugue-jam/"
package_size="$(du -sb "$arch_stage" | cut -f1)"
cat > "$arch_stage/.PKGINFO" <<EOF
pkgname = fugue-jam
pkgbase = fugue-jam
pkgver = $version-1
pkgdesc = A local-first Linux music player
url = https://github.com/mpgauley/Fugue-Jam
builddate = $(date +%s)
packager = Fugue Jam contributors
size = $package_size
arch = x86_64
license = Apache-2.0
depend = alsa-lib
depend = fontconfig
optdepend = noto-fonts-cjk: multilingual song titles
EOF
tar --zstd --format=gnu -C "$arch_stage" \
  -cf "$output/fugue-jam-$version-1-x86_64.pkg.tar.zst" .

(
  cd "$output"
  sha256sum ./*.deb ./*.rpm ./*.pkg.tar.zst > SHA256SUMS
)
