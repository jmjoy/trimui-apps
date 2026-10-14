set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments

mod chinese_chess "crates/chinese-chess/justfile"
mod gomoku "crates/gomoku/justfile"
mod voxel_world "crates/voxel-world/justfile"

scripts := justfile_directory() / "scripts"

default: list

list:
    @just --justfile "{{ justfile_directory() }}/justfile" --list --list-submodules

fmt project="all":
    bash "{{ scripts }}/cargo.sh" fmt "$@"

test project="all":
    bash "{{ scripts }}/cargo.sh" test "$@"

clippy project="all":
    bash "{{ scripts }}/cargo.sh" clippy "$@"

check project="all": (fmt project) (test project) (clippy project)

build project="all" profile="release":
    bash "{{ scripts }}/cargo.sh" build "$@"

[private]
_stage project profile: (build project profile)
    bash "{{ scripts }}/stage.sh" "$@"

package project="all" profile="release": (_stage project profile)
    bash "{{ scripts }}/package.sh" "$1"

deploy project="all" profile="release": (package project profile)
    bash "{{ scripts }}/deploy.sh" "$1"

clean project="all":
    bash "{{ scripts }}/clean.sh" "$@"
