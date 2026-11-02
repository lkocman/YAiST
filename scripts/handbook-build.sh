#!/bin/bash
# Builds the openSUSE Handbook (docs/handbook) into OUT (default
# target/handbook) as the formats given (default: pdf epub html). Runs on
# openSUSE Tumbleweed; with --install it first installs the tools:
# asciidoctor from the distribution, its PDF and EPUB backends from
# rubygems (Tumbleweed has no packages of them), inter-fonts and fontTools.
# Used by CI and, in a container, by scripts/build-handbook.sh.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
if [ "${1:-}" = --install ]; then
    shift
    zypper -nq --gpg-auto-import-keys install --no-recommends \
        ruby ruby-devel "rubygem(asciidoctor)" "rubygem(rouge)" gcc make inter-fonts python3-fonttools
    gem install --no-document --silent --bindir /usr/local/bin --no-format-executable asciidoctor-pdf asciidoctor-epub3
fi
out="$(realpath -m "${1:-$root/target/handbook}")"
shift || true
[ $# -gt 0 ] || set -- pdf epub html
mkdir -p "$out"
fonts="$(mktemp -d)"
trap 'rm -rf "$fonts"' EXIT
cd "$root/docs/handbook"
for f in "$@"; do
    case $f in
        pdf)
            python3 "$root/scripts/handbook-fonts.py" "$fonts"
            asciidoctor-pdf --failure-level WARN -a pdf-fontsdir="$fonts;GEM_FONTS_DIR" -o "$out/opensuse-handbook.pdf" opensuse-handbook.adoc
            ;;
        epub) asciidoctor-epub3 --failure-level WARN -a ebook-validate! -o "$out/opensuse-handbook.epub" opensuse-handbook.adoc ;;
        html)
            asciidoctor --failure-level WARN -a imagesdir=screenshots -o "$out/index.html" opensuse-handbook.adoc
            rm -rf "$out/screenshots" && cp -r ../screenshots "$out/"
            ;;
        *) echo "unknown format: $f (pdf, epub, html)" >&2; exit 2 ;;
    esac
done
echo "built $* in $out"
