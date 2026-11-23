<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/slim-dark.svg">
    <img src="assets/logo/slim-light.svg" alt="slim" height="72">
  </picture>
</h1>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/screenshots/reading-dark.gif">
    <img src="assets/screenshots/reading-light.gif" alt="Fifteen seconds of slim on the Lean proof of VP ≠ VNP: the theorems the project ends in, the main theorem with its statement unfolded, what the proof rests on, and why one lemma is there.">
  </picture>
</p>

**slim is a terminal reader for large Lean 4 projects.** Open a proof you have never seen, and
it shows what the project proves, what each step rests on, and any statement with the
project's own definitions unfolded under it.

It needs no Lean installation, and it never runs anything from the project it reads.

## Try it

Three steps on Linux, under a minute:

```sh
# 1. Get slim
name=slim-v0.2.0-$(uname -m)-unknown-linux-gnu
curl -fL https://github.com/tzu-chen/slim/releases/download/v0.2.0/$name.tar.gz | tar -xz
export PATH="$PWD/$name:$PATH"

# 2. Get a proof: VP ≠ VNP, about 10 MB
git clone https://github.com/anthropics/formal-math.git

# 3. Read it
slim formal-math/vp-vnp
```

Then do what the animation does:

1. slim opens on the five theorems the project ends in. `Enter` opens the main theorem.
2. `u` unfolds its statement, one level of the project's definitions at a time.
3. `D` lists what the proof rests on, largest parts first. `Space` unfolds a row.
4. `W` shows why the selected row is there: a chain of references down from a main theorem.

`/` searches, `Backspace` goes back, `?` shows every key, `q` quits.

The binaries are for `x86_64` and `aarch64` Linux with glibc 2.34 or newer; archives and
checksums are on the [release page](https://github.com/tzu-chen/slim/releases/tag/v0.2.0). On
anything else, [build from source](#build-from-source). The `export` line lasts for this
terminal: copy the `slim` file to a directory on your `PATH` to keep it.

## More to try

* `slim demo` opens a small library that is built into the binary. Nothing to download.
* `slim path/to/your/project` opens any Lean 4 project. The first start reads its `.lean`
  files (about two seconds for 10,000 files) and keeps what it learns in `~/.cache/slim`, never
  in the project.
* [A session on Yang–Mills](docs/guide.md#a-first-session-on-a-real-project-ym): 9,564 files,
  read through a project map.

## What slim tells you, and what it does not

slim is not a proof checker. It never says a theorem is "proved": it shows evidence and where
the evidence came from. Without a Lean build it matches names in the source text, and every
screen says so with `[lex]`. With a build it reads Lean's own reference data instead;
[docs/reading.md](docs/reading.md) has the details.

## Build from source

You need a Rust toolchain (tested with rustc 1.99.1) and a C compiler.

```sh
git clone https://github.com/tzu-chen/slim.git && cd slim
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## Documentation

* [User guide](docs/guide.md): the [VP ≠ VNP session](docs/guide.md#a-new-proof-read-top-down-vp--vnp)
  in full, the [demo](docs/guide.md#the-bundled-demo), [your own projects](docs/guide.md#your-own-projects)
  and [every command](docs/guide.md#commands)
* [Reading a large proof](docs/reading.md): how the views above work
* [What slim runs, and what it never runs](docs/guide.md#trust-and-execution-boundaries)

## License

Apache-2.0, see [LICENSE](LICENSE). Bundled third-party grammars and themes and their licences
are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
