# Project Dependencies

This is a [Rust programming language](https://rust-lang.org/) project requiring the [Rust toolchain](https://rust-lang.org/tools/install/), including the Rust build system and package manager, named `cargo`.

In addition to the base toolchain, this project also makes use of the following:

| Tool                                          | Description                                           | Rationale                                                 |
|-----------------------------------------------|-------------------------------------------------------|-----------------------------------------------------------|
| [uv](https://docs.astral.sh/uv/)              | An extremely fast Python package and project manager  | Solely for benchmarking against the Python implementation |
| [just](https://just.systems/man/en/)          | A handy way to save and run project-specific commands | Convenience                                               |
| [jq](https://jqlang.org/)                     | A handy way to slice and filter JSON output           | Convenience tool for JSON and JSONL.                      |
| [mdbook](https://rust-lang.github.io/mdBook/) | A tool to create books with Markdown.                 | This project is documented via mdbook.                    |



