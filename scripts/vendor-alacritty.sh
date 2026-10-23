#!/usr/bin/env bash
# Manage the vendored alacritty_terminal subtree in crates/alacritty_terminal.
#
# The upstream alacritty repo is a monorepo, so we first split the
# alacritty_terminal subdirectory into its own branch inside a helper clone
# (kept outside this repository), then merge that branch here with
# `git subtree` so the vendored files become ordinary tracked files.
#
# Usage:
#   scripts/vendor-alacritty.sh setup             Clone or refresh the upstream helper clone
#   scripts/vendor-alacritty.sh import v0.26.0    First-time vendor into crates/alacritty_terminal
#   scripts/vendor-alacritty.sh pull v0.27.0      Merge a newer upstream tag into the vendor dir
#
# Requires: git with the subtree contrib command, clean working tree.
# Override the helper clone location with ALACRITTY_UPSTREAM_DIR.

set -euo pipefail

UPSTREAM_URL="https://github.com/alacritty/alacritty.git"
UPSTREAM_PREFIX="alacritty_terminal"
VENDOR_PREFIX="crates/alacritty_terminal"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPSTREAM_DIR="${ALACRITTY_UPSTREAM_DIR:-$REPO_ROOT/../alacritty-upstream}"

usage() {
    cat <<EOF
Usage: $0 <command> [args]

Commands:
  setup              Clone or refresh the upstream helper clone
  import <tag>       First-time vendor of alacritty_terminal (e.g. v0.26.0)
  pull <tag>         Merge a newer upstream tag into the vendored directory

Environment:
  ALACRITTY_UPSTREAM_DIR  Helper clone location (default: ../alacritty-upstream)
EOF
}

die() { echo "error: $*" >&2; exit 1; }

require_subtree() {
    # `git subtree -h` exits non-zero even when subtree IS installed (the usage
    # goes to stderr and the command exits 1/129), so probe for the executable
    # instead of trusting the exit code.
    if ! type -p git-subtree >/dev/null 2>&1 \
        && [[ ! -x "$(git --exec-path)/git-subtree" ]]; then
        die "git-subtree is not installed (a contrib command shipped with git).
     On MSYS2: cp /usr/share/git-core/contrib/subtree/git-subtree.sh \$(git --exec-path)/git-subtree"
    fi
}

require_clean_tree() {
    git -C "$REPO_ROOT" diff --quiet HEAD \
        || die "working tree is dirty; commit or stash changes first"
}

ensure_upstream() {
    if [[ -d "$UPSTREAM_DIR/.git" ]]; then
        echo "Refreshing upstream clone at $UPSTREAM_DIR ..."
        git -C "$UPSTREAM_DIR" fetch --tags origin
    else
        echo "Cloning upstream into $UPSTREAM_DIR ..."
        git clone "$UPSTREAM_URL" "$UPSTREAM_DIR"
    fi
}

split_tag() {
    local version="$1"
    local branch="terminal-$version"

    git -C "$UPSTREAM_DIR" rev-parse --verify --quiet "refs/tags/$version" >/dev/null \
        || die "upstream tag $version not found (run: git -C \"$UPSTREAM_DIR\" tag -l)"

    git -C "$UPSTREAM_DIR" branch -D "$branch" >/dev/null 2>&1 || true
    # `git subtree split` prints the split commit SHA on stdout; redirect it to
    # stderr so callers capture only the branch name from this function.
    git -C "$UPSTREAM_DIR" subtree split --prefix="$UPSTREAM_PREFIX" "$version" -b "$branch" >&2
    echo "$branch"
}

cmd_setup() {
    ensure_upstream
    echo "Upstream clone ready at $UPSTREAM_DIR"
}

cmd_import() {
    local version="${1:?usage: import <tag>}"

    require_subtree
    require_clean_tree

    if [[ -e "$REPO_ROOT/$VENDOR_PREFIX" ]]; then
        die "$VENDOR_PREFIX already exists; use 'pull' to update"
    fi

    ensure_upstream
    local branch
    branch="$(split_tag "$version")"

    git -C "$REPO_ROOT" subtree add --prefix="$VENDOR_PREFIX" \
        "$UPSTREAM_DIR" "$branch" --squash

    echo "vendored alacritty_terminal $version into $VENDOR_PREFIX"
}

cmd_pull() {
    local version="${1:?usage: pull <tag>}"

    require_subtree
    require_clean_tree

    if [[ ! -e "$REPO_ROOT/$VENDOR_PREFIX" ]]; then
        die "$VENDOR_PREFIX does not exist; run 'import' first"
    fi

    ensure_upstream
    local branch
    branch="$(split_tag "$version")"

    git -C "$REPO_ROOT" subtree pull --prefix="$VENDOR_PREFIX" \
        "$UPSTREAM_DIR" "$branch" --squash

    echo "merged alacritty_terminal $version into $VENDOR_PREFIX"
}

# ---------------------------------------------------------------------------
case "${1:-}" in
    setup) cmd_setup ;;
    import) shift; cmd_import "$@" ;;
    pull)   shift; cmd_pull "$@" ;;
    *) usage; exit 1 ;;
esac
