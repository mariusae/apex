#!/bin/sh
# Build Apex.app into target/. Usage: mac/build-app.sh [--debug]
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
profile=release; flag=--release
if [ "$1" = "--debug" ]; then profile=debug; flag=; fi
cargo build $flag -p apex-client -p apex-cli
# the command for Linux hosts (attached over ssh); Zig is the cross linker
cargo build --release --target x86_64-unknown-linux-musl -p apex-cli

app=target/Apex.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/apex-ui" "$app/Contents/MacOS/apex-ui"
cp "target/$profile/apex" "$app/Contents/MacOS/apex"
mkdir -p "$app/Contents/Resources/remote/linux-amd64"
cp target/x86_64-unknown-linux-musl/release/apex "$app/Contents/Resources/remote/linux-amd64/apex"
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
sed "s/VERSION/$version/g" mac/Info.plist > "$app/Contents/Info.plist"
echo -n "APPL????" > "$app/Contents/PkgInfo"

# the icon: rasterize the SVG once (needs Chrome), then every size macOS wants
if [ ! -f mac/glenda-1024.png ]; then
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars \
    --default-background-color=00000000 --window-size=1024,1024 \
    --screenshot="$PWD/mac/glenda-1024.png" "file://$PWD/mac/glenda.svg" >/dev/null 2>&1
fi
iconset=target/apex.iconset
rm -rf "$iconset"; mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s mac/glenda-1024.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s*2))
  sips -z $d $d mac/glenda-1024.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/apex.icns"

# ad-hoc signature so Gatekeeper lets a local build launch
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app"
