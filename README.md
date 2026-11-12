<p align="center"><img src="docs/space-bunny.png" alt="The Apex space bunny" width="200"></p>

# Apex

Apex is a modernized [acme](https://en.wikipedia.org/wiki/Acme_(text_editor)):
text first, mouse oriented, with plumbing everywhere. It adopts acme's way
of working, and then adds what modern software engineering workflows
increasingly require: remoting, terminals, and web views. Apex ships as a
single Mac app; the daemon and the `apex` command also run on Linux, where
the app attaches to them over ssh.

- [mariusae.github.io/apex](https://mariusae.github.io/apex/): what Apex is, and how it differs from acme.
- [The user's guide](https://mariusae.github.io/apex/userguide.html): how to use it.
- [The design and implementation wiki](https://mariusae.github.io/apex/wiki/html/index.html): how it works.

![Apex with two columns: a file being edited, a preview of it, and a terminal running a coding agent.](docs/screenshot.png)

## Building

Apex is written in Rust using [GPUI](https://gpui.rs). To build the app:

    % git clone https://github.com/mariusae/apex
    % cd apex
    % mac/build-app.sh

This produces `target/Apex.app`. Once it is running, *Apex ▸ Install apex
Command…* puts `apex` on your path.

## Further reading

- [DESIGN.md](DESIGN.md) describes how Apex is built, and why.
- [MODERN.md](MODERN.md) describes Apex as a Mac app.
- [WEB.md](WEB.md) describes web windows and Preview.
- [ARCHITECTURE.md](ARCHITECTURE.md) is a review of the architecture.

Apex is a work in progress. Alien Space Bunny is by Wilhelm Bierbaum, based
on artwork by Renée French.

## License

Apex is under the [MIT license](LICENSE). The parts ported from
plan9port's acme and sam (the Edit language in `crates/apex-edit`, the
tiling in `crates/apex-core/src/tiling.rs`) are also under plan9port's
MIT license, reproduced in the same file.

What Apex bundles or builds in keeps its own license: the fonts, Nerd
Fonts' symbols, Mermaid and pjw's face, as listed in
`crates/apex-client/assets/README.md`; `third_party/pathfinder_simd`
(MIT or Apache-2.0); Ghostty's terminal library, built from source by
`crates/ghostty-vt-sys` (MIT); and plan9port's `rc`, in the app (MIT).
The Alien Space Bunny -- the icon and its drawing (`mac/space-bunny.*`,
`mac/apex.icon`, `docs/space-bunny.png`,
`crates/apex-client/assets/space-bunny.svg`) -- is Wilhelm Bierbaum's,
after Renée French's artwork, and is not under the MIT license.
