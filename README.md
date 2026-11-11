# slim

A fast terminal navigator for **reading** large Lean 4 projects: find a theorem, read its
statement, documentation and source, follow what it depends on and what depends on it, and get
the big picture of a project through a curated map, statement-only skims and the import order.
Editing and developing proofs stays in your editor.

slim is not a proof checker. It never claims a theorem is "proved"; it shows evidence and where
that evidence came from.

## Build

You need a Rust toolchain (tested with rustc 1.99.1) and a C compiler (for the bundled SQLite
and Oniguruma). Dependencies are pinned in `Cargo.lock`.

```sh
git clone https://github.com/tzu-chen/slim.git && cd slim
cargo build --release
```

The binary is `target/release/slim`; put that directory on your `PATH` or copy the file. Nothing
else is needed to read a project: no Lean installation, no Lake, no network.

## Run the demo

```sh
./target/release/slim demo
```

This opens a miniature Lean library that is embedded in the binary together with its semantic
data (real output of Lean 4.33.1), so it needs no Lean, no network and no compilation. It opens
on the library's map.

| Key | |
|---|---|
| `j` `k` | move |
| `Enter` | open the selected declaration |
| `/` | search by name or by words from the documentation |
| `1` `2` `3` | what a declaration uses, what uses it, its axioms with evidence paths |
| `Backspace` | go back |
| `?` | every key |
| `q` | quit |

## Try it on a real project

The public [YM repository](https://github.com/yang-mills-group/YM) has 9,564 Lean files, and
slim ships a project map for it in `examples/YM/lean-view.toml`:

```sh
git clone https://github.com/yang-mills-group/YM.git      # about 160 MB; nothing is built
./target/release/slim YM --map examples/YM/lean-view.toml
```

The first start scans the `.lean` files as text (about a second) and keeps its index in
`~/.cache/slim`, never in the checkout. It runs no Lean and no project code. slim opens on the
map: `j`/`k` move, `Enter` opens, `*` pins a goal and `A` unfolds the map's argument for it,
`S` skims the selected component, `T` lists reading trails.

The YM map is an example of the format, drafted by an AI assistant and not accepted by anyone;
slim marks every entry of it as a suggestion. It is a reading aid, not a statement about the
mathematics.

## Where to go next

The [user guide](docs/guide.md) continues from here:

* [a step-by-step first session on YM](docs/guide.md#a-first-session-on-a-real-project-ym)
* [tours of the demo](docs/guide.md#the-bundled-demo) and of [its map](docs/guide.md#the-demos-map)
* [opening your own projects](docs/guide.md#your-own-projects), source-only or with Lean's own
  dependency data
* [what slim runs, and what it never runs](docs/guide.md#trust-and-execution-boundaries)
* [supported Lean toolchains](docs/guide.md#supported-lean-toolchains),
  [every command](docs/guide.md#commands) and [the other documents](docs/guide.md#other-documents)

## License

Apache-2.0, see [LICENSE](LICENSE). Bundled third-party grammars and themes and their licences
are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
