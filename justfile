set shell := ["bash", "-euo", "pipefail", "-c"]

mod chinese_chess "crates/chinese-chess/justfile"
mod gomoku "crates/gomoku/justfile"
mod voxel_world "crates/voxel-world/justfile"

default: list

list:
    @just --list --list-submodules

fmt:
    cargo fmt --manifest-path "{{ justfile_directory() }}/Cargo.toml" --all -- --check

test:
    cargo test --manifest-path "{{ justfile_directory() }}/Cargo.toml" --locked --workspace

clippy:
    cargo clippy --manifest-path "{{ justfile_directory() }}/Cargo.toml" --locked --workspace --all-targets -- -D warnings

check: fmt test clippy

build project="chinese_chess" profile="release":
    just {{ project }}::build "{{ profile }}"

package project="chinese_chess" profile="release":
    just {{ project }}::package "{{ profile }}"

deploy project="chinese_chess" profile="release":
    just {{ project }}::deploy "{{ profile }}"

run project="chinese_chess" *args:
    just {{ project }}::run {{ args }}

input-test project="chinese_chess":
    just {{ project }}::input-test

bench project="voxel_world" seconds="40":
    just {{ project }}::bench "{{ seconds }}"

exit-test project="voxel_world":
    just {{ project }}::exit-test

clean project="chinese_chess":
    just {{ project }}::clean
