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
# rc, the shell commands run with (mariusae/rustrc), for this machine and for Linux hosts;
# cargo install builds out of tree, so the cross linker goes in through the environment
rc_git=https://github.com/mariusae/rustrc
cargo install --git $rc_git --bin rc --root target/rc-host --force --quiet
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$PWD/mac/zig-cc" \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no" \
  cargo install --git $rc_git --bin rc --root target/rc-linux-amd64 --target x86_64-unknown-linux-musl --force --quiet

app=target/Apex.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/apex-ui" "$app/Contents/MacOS/apex-ui"
cp "target/$profile/apex" "$app/Contents/MacOS/apex"
ln -sfn apex "$app/Contents/MacOS/apex-editor"
ln -sfn apex "$app/Contents/MacOS/xdg-open"
cp target/rc-host/bin/rc "$app/Contents/MacOS/rc"
mkdir -p "$app/Contents/Resources/remote/linux-amd64"
cp target/x86_64-unknown-linux-musl/release/apex "$app/Contents/Resources/remote/linux-amd64/apex"
cp target/rc-linux-amd64/bin/rc "$app/Contents/Resources/remote/linux-amd64/rc"
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
sed "s/VERSION/$version/g" mac/Info.plist > "$app/Contents/Info.plist"
echo -n "APPL????" > "$app/Contents/PkgInfo"

# the icon: the space bunny (mac/space-bunny.svg, drawn at 1024 on
# nothing). macOS 26 and later set a bundle's icon in a rounded square
# of their own unless it is one already, so the bundle's is an Icon
# Composer icon (mac/apex.icon: the sticker on navy) compiled by Xcode's
# actool -- Assets.car, and apex.icns for older systems. The app sets the
# sticker alone as its Dock icon while it runs. Without Xcode, the
# sticker alone at every size, set in the system's square.
icon=target/apex.icon
rm -rf "$icon"; mkdir -p "$icon/Assets"
cp mac/apex.icon/icon.json "$icon/"
cp mac/space-bunny-1024.png "$icon/Assets/space-bunny.png"
# (absolute paths: actool's helper may not run where we are)
if ! xcrun actool "$PWD/$icon" --compile "$(cd "$app/Contents/Resources" && pwd)" --app-icon apex --platform macosx \
     --minimum-deployment-target 12.0 --output-partial-info-plist "$PWD/target/apex-icon.plist" >/dev/null 2>&1; then
  iconset=target/apex.iconset
  rm -rf "$iconset"; mkdir -p "$iconset"
  for s in 16 32 128 256 512; do
    sips -z $s $s mac/space-bunny-1024.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s*2))
    sips -z $d $d mac/space-bunny-1024.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$app/Contents/Resources/apex.icns"
fi

# ad-hoc signature so Gatekeeper lets a local build launch
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app"
